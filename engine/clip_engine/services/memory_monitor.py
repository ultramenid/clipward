"""
Memory monitoring utility for Fargate-optimized video processing.

Provides memory usage tracking and garbage collection helpers to prevent OOM
on constrained environments (4 vCPU / 8 GB RAM).

Usage:
    from clip_engine.services.memory_monitor import log_memory_usage, force_gc

    # Log current memory usage at key pipeline stages
    log_memory_usage("after_download")

    # Force garbage collection to free memory
    force_gc("after_detection")
"""

import gc
import logging
import os
import sys
from typing import Optional

logger = logging.getLogger(__name__)

# Try to import psutil for accurate memory measurement
# Falls back to resource module on Linux if psutil not available
_psutil_available = False
_resource_available = False

try:
    import psutil
    _psutil_available = True
except ImportError:
    pass

if not _psutil_available:
    try:
        import resource
        _resource_available = True
    except ImportError:
        pass


def get_memory_usage_mb() -> dict:
    """
    Get current process memory usage in MB.
    
    Returns:
        Dict with 'rss' (Resident Set Size) and 'vms' (Virtual Memory Size)
        Returns {'rss': 0, 'vms': 0} if measurement fails
    """
    if _psutil_available:
        try:
            process = psutil.Process(os.getpid())
            mem_info = process.memory_info()
            return {
                "rss": mem_info.rss / (1024 * 1024),  # Convert to MB
                "vms": mem_info.vms / (1024 * 1024),
            }
        except Exception:
            pass
    
    if _resource_available:
        try:
            import resource
            # getrusage reports ru_maxrss in KB on Linux but bytes on macOS
            usage = resource.getrusage(resource.RUSAGE_SELF)
            unit = 1024 * 1024 if sys.platform == "darwin" else 1024
            return {
                "rss": usage.ru_maxrss / unit,
                "vms": 0,  # Not available via resource module
            }
        except Exception:
            pass
    
    return {"rss": 0, "vms": 0}


def log_memory_usage(stage: str, job_id: Optional[str] = None) -> dict:
    """
    Log current memory usage at a pipeline stage.
    
    Args:
        stage: Name of the pipeline stage (e.g., "after_download", "after_detection")
        job_id: Optional job ID for log correlation
        
    Returns:
        Memory usage dict with 'rss' and 'vms' in MB
    """
    mem = get_memory_usage_mb()
    
    prefix = f"[{job_id}] " if job_id else ""
    
    if mem["rss"] > 0:
        logger.info(
            f"{prefix}Memory [{stage}]: RSS={mem['rss']:.1f}MB, VMS={mem['vms']:.1f}MB"
        )
        
        # Warn if memory usage is high (> 6GB on 8GB container)
        if mem["rss"] > 6000:
            logger.warning(
                f"{prefix}HIGH MEMORY WARNING [{stage}]: RSS={mem['rss']:.1f}MB exceeds 6GB threshold"
            )
    else:
        logger.debug(f"{prefix}Memory [{stage}]: measurement unavailable (psutil not installed)")
    
    return mem


def force_gc(stage: str, job_id: Optional[str] = None) -> dict:
    """
    Force garbage collection and log memory before/after.
    
    Use this between major pipeline stages to free memory from
    completed operations (e.g., after download, after detection).
    
    Args:
        stage: Name of the pipeline stage
        job_id: Optional job ID for log correlation
        
    Returns:
        Memory usage dict after GC
    """
    prefix = f"[{job_id}] " if job_id else ""
    
    mem_before = get_memory_usage_mb()
    
    # Run full garbage collection (all generations)
    collected = gc.collect()
    
    mem_after = get_memory_usage_mb()
    
    freed = mem_before["rss"] - mem_after["rss"]
    
    if mem_before["rss"] > 0:
        logger.info(
            f"{prefix}GC [{stage}]: collected {collected} objects, "
            f"freed {freed:.1f}MB (RSS: {mem_before['rss']:.1f}MB -> {mem_after['rss']:.1f}MB)"
        )
    else:
        logger.debug(f"{prefix}GC [{stage}]: collected {collected} objects")
    
    return mem_after


def get_memory_limit_mb() -> Optional[float]:
    """
    Get container memory limit (for Fargate/Docker).
    
    Returns:
        Memory limit in MB, or None if not running in a container
    """
    # Check cgroups v2 first (newer systems)
    cgroup_v2_path = "/sys/fs/cgroup/memory.max"
    if os.path.exists(cgroup_v2_path):
        try:
            with open(cgroup_v2_path, "r") as f:
                value = f.read().strip()
                if value != "max":
                    return int(value) / (1024 * 1024)
        except Exception:
            pass
    
    # Check cgroups v1 (older systems)
    cgroup_v1_path = "/sys/fs/cgroup/memory/memory.limit_in_bytes"
    if os.path.exists(cgroup_v1_path):
        try:
            with open(cgroup_v1_path, "r") as f:
                value = int(f.read().strip())
                # Check if it's not the "unlimited" sentinel value
                if value < 9223372036854771712:
                    return value / (1024 * 1024)
        except Exception:
            pass
    
    return None


def check_memory_pressure(threshold_percent: float = 75.0) -> bool:
    """
    Check if memory usage is above threshold.
    
    Args:
        threshold_percent: Percentage of memory limit to consider as pressure
        
    Returns:
        True if memory pressure is high, False otherwise
    """
    mem = get_memory_usage_mb()
    limit = get_memory_limit_mb()
    
    if limit and mem["rss"] > 0:
        usage_percent = (mem["rss"] / limit) * 100
        if usage_percent > threshold_percent:
            logger.warning(
                f"Memory pressure: {usage_percent:.1f}% of {limit:.0f}MB limit "
                f"(threshold: {threshold_percent}%)"
            )
            return True
    
    # Fallback: check against 6GB absolute threshold (for 8GB Fargate)
    if mem["rss"] > 6000:
        logger.warning(f"Memory pressure: RSS {mem['rss']:.1f}MB exceeds 6GB threshold")
        return True
    
    return False
