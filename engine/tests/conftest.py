"""
Pytest configuration and fixtures.
"""

import os
import sys

import pytest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))


@pytest.fixture
def mock_settings():
    """Mock settings for testing."""
    return {
        "aws_region": "us-east-1",
        "s3_bucket": "test-bucket",
        "temp_directory": "/tmp/test-worker",
        "max_workers": 4,
        "max_render_workers": 3,
    }
