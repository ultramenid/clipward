"""
S3 Upload Service - Handles uploading clips and artifacts to S3.
"""

import asyncio
import json
import logging
import os
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

import boto3
from boto3.s3.transfer import TransferConfig
from botocore.config import Config as BotocoreConfig
from botocore.exceptions import ClientError

from clip_engine.config import get_settings

logger = logging.getLogger(__name__)


@dataclass
class UploadResult:
    """Result of S3 upload operation."""
    
    s3_url: str
    bucket: str
    key: str
    file_size_bytes: int
    content_type: str


@dataclass
class ClipArtifact:
    """A clip artifact with metadata."""
    
    clip_index: int
    s3_url: str
    duration_ms: int
    start_time_ms: int
    end_time_ms: int
    virality_score: float
    layout_type: str
    summary: Optional[str] = None
    tags: list[str] = None
    # Set when the smart render failed and a letterbox fallback produced the
    # clip ("letterbox" keeps pacing cuts, "letterbox_natural" doesn't).
    render_fallback: Optional[str] = None
    # Longform clips: upload description, chapters ({"time_ms", "title"} on
    # the clip's own timeline) and the SRT sidecar (local mode only).
    description: Optional[str] = None
    chapters: Optional[list[dict]] = None
    subtitle_url: Optional[str] = None


@dataclass
class JobOutput:
    """Complete output of an AI clipping job."""

    job_id: str
    source_video_url: str
    source_video_title: str
    source_video_duration_seconds: float  # Source video duration for credit calculation
    total_clips: int
    clips: list[ClipArtifact]
    user_id: Optional[str] = None  # User ID for S3 key scoping
    transcript_url: Optional[str] = None
    plan_url: Optional[str] = None
    processing_time_seconds: float = 0
    metrics: Optional[dict[str, Any]] = None
    created_at: str = None
    
    def __post_init__(self):
        if self.created_at is None:
            self.created_at = datetime.now(timezone.utc).isoformat()


class S3UploadService:
    """
    Service for uploading clips and artifacts to S3.
    
    Features:
    - Multipart upload for large files
    - Concurrent uploads
    - Metadata attachment
    - URL generation
    """

    def __init__(self):
        self.settings = get_settings()
        self._client: Optional[boto3.client] = None
        self._transfer_config = TransferConfig(
            multipart_threshold=8 * 1024 * 1024,
            multipart_chunksize=8 * 1024 * 1024,
            max_concurrency=self.settings.s3_transfer_max_concurrency,
            use_threads=True,
        )

    @property
    def client(self) -> boto3.client:
        """Lazy-initialize S3 client."""
        if self._client is None:
            config = {
                "region_name": self.settings.aws_region,
                "config": BotocoreConfig(
                    retries={"max_attempts": 10, "mode": "adaptive"},
                    max_pool_connections=self.settings.s3_max_pool_connections,
                    connect_timeout=self.settings.s3_connect_timeout_seconds,
                    read_timeout=self.settings.s3_read_timeout_seconds,
                ),
            }
            if self.settings.aws_access_key_id and self.settings.aws_secret_access_key:
                config["aws_access_key_id"] = self.settings.aws_access_key_id
                config["aws_secret_access_key"] = self.settings.aws_secret_access_key
            
            self._client = boto3.client("s3", **config)
        
        return self._client

    async def upload_clip(
        self,
        local_path: str,
        job_id: str,
        clip_index: int,
        user_id: Optional[str] = None,
        metadata: Optional[dict[str, Any]] = None,
    ) -> UploadResult:
        """
        Upload a rendered clip to S3.
        
        Args:
            local_path: Path to local clip file
            job_id: Job identifier
            clip_index: Clip index for naming
            user_id: User ID for S3 key scoping (required for authorized access)
            metadata: Optional metadata to attach
            
        Returns:
            UploadResult with S3 URL and details
        """
        if not os.path.isfile(local_path):
            raise S3UploadError(f"File not found: {local_path}")
        
        # Generate S3 key using the format expected by MediaAuthorizationService
        # Format: ai-clipping-agent/{userId}/{jobId}/{filename}
        timestamp = datetime.utcnow().strftime("%Y%m%d%H%M%S")
        filename = f"clip_{clip_index:02d}_{timestamp}.mp4"
        
        if user_id:
            # Use user-scoped path for authorized access via media proxy
            s3_key = f"ai-clipping-agent/{user_id}/{job_id}/{filename}"
        else:
            # Fallback to legacy format (won't work with media proxy auth)
            s3_key = f"clips/{timestamp[:8]}/{job_id}/{filename}"
            logger.warning(f"No user_id provided for upload - using legacy path format")
        
        logger.info(f"Uploading clip to s3://{self.settings.s3_bucket}/{s3_key}")
        
        file_size = os.path.getsize(local_path)
        
        # Prepare upload metadata
        extra_args = {
            "ContentType": "video/mp4",
        }
        if metadata:
            # S3 metadata values must be strings
            extra_args["Metadata"] = {
                k: str(v) for k, v in metadata.items()
            }
        
        # Upload (use thread pool for sync boto3 call)
        loop = asyncio.get_event_loop()
        await loop.run_in_executor(
            None,
            lambda: self.client.upload_file(
                local_path,
                self.settings.s3_bucket,
                s3_key,
                ExtraArgs=extra_args,
                Config=self._transfer_config,
            ),
        )
        
        # Generate URL
        s3_url = f"https://{self.settings.s3_bucket}.s3.{self.settings.aws_region}.amazonaws.com/{s3_key}"
        
        logger.info(f"Upload complete: {s3_url}")
        
        return UploadResult(
            s3_url=s3_url,
            bucket=self.settings.s3_bucket,
            key=s3_key,
            file_size_bytes=file_size,
            content_type="video/mp4",
        )

    async def upload_json_artifact(
        self,
        data: dict[str, Any],
        job_id: str,
        artifact_name: str,
        user_id: Optional[str] = None,
    ) -> UploadResult:
        """
        Upload a JSON artifact (transcript, plan, etc.) to S3.
        
        Args:
            data: Dictionary to serialize as JSON
            job_id: Job identifier
            artifact_name: Name of artifact (e.g., 'transcript', 'plan')
            user_id: User ID for S3 key scoping
            
        Returns:
            UploadResult with S3 URL
        """
        timestamp = datetime.utcnow().strftime("%Y%m%d")
        
        if user_id:
            # Use user-scoped path for authorized access
            s3_key = f"ai-clipping-agent/{user_id}/{job_id}/{artifact_name}.json"
        else:
            # Fallback to legacy format
            s3_key = f"clips/{timestamp}/{job_id}/{artifact_name}.json"
        
        json_content = json.dumps(data, separators=(",", ":"), sort_keys=True)
        
        loop = asyncio.get_event_loop()
        await loop.run_in_executor(
            None,
            lambda: self.client.put_object(
                Bucket=self.settings.s3_bucket,
                Key=s3_key,
                Body=json_content.encode("utf-8"),
                ContentType="application/json",
            ),
        )
        
        s3_url = f"https://{self.settings.s3_bucket}.s3.{self.settings.aws_region}.amazonaws.com/{s3_key}"
        
        return UploadResult(
            s3_url=s3_url,
            bucket=self.settings.s3_bucket,
            key=s3_key,
            file_size_bytes=len(json_content),
            content_type="application/json",
        )

    async def upload_job_output(
        self,
        output: JobOutput,
    ) -> UploadResult:
        """
        Upload the final job output manifest to S3.
        
        Args:
            output: JobOutput with all clip details
            
        Returns:
            UploadResult for the manifest file
        """
        # Convert to dict, handling dataclass conversions
        output_dict = asdict(output)
        
        return await self.upload_json_artifact(
            data=output_dict,
            job_id=output.job_id,
            artifact_name="job_output",
            user_id=output.user_id,
        )

    async def generate_presigned_url(
        self,
        s3_key: str,
        expiration_seconds: int = 3600,
    ) -> str:
        """
        Generate a presigned URL for downloading a file.
        
        Args:
            s3_key: S3 object key
            expiration_seconds: URL validity duration
            
        Returns:
            Presigned download URL
        """
        loop = asyncio.get_event_loop()
        url = await loop.run_in_executor(
            None,
            lambda: self.client.generate_presigned_url(
                "get_object",
                Params={
                    "Bucket": self.settings.s3_bucket,
                    "Key": s3_key,
                },
                ExpiresIn=expiration_seconds,
            ),
        )
        return url

    async def check_file_exists(self, s3_key: str) -> bool:
        """Check if a file exists in S3."""
        try:
            loop = asyncio.get_event_loop()
            await loop.run_in_executor(
                None,
                lambda: self.client.head_object(
                    Bucket=self.settings.s3_bucket,
                    Key=s3_key,
                ),
            )
            return True
        except ClientError as e:
            if e.response["Error"]["Code"] == "404":
                return False
            raise


class S3UploadError(Exception):
    """Exception raised when S3 upload fails."""
    pass
