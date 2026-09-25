#!/usr/bin/env bash
set -euo pipefail

# Export only files named in public-draft-manifest.txt. Use a new destination so
# this command cannot erase a previous review or write inside the source checkout.
destination="${1:?Pass a new draft directory outside the Clipward source repository}"
bridge_source="$(cd "$(dirname "$0")/.." && pwd)"

python3 - "$bridge_source" "$destination" <<'PY'
import os
import shutil
import stat
import sys
from pathlib import Path

bridge_source = Path(sys.argv[1]).resolve(strict=True)
requested_destination = Path(sys.argv[2]).expanduser().absolute()
if requested_destination.exists() or requested_destination.is_symlink():
    raise SystemExit("Public draft export refused: choose a new destination path")
destination = requested_destination.resolve(strict=False)
manifest_path = bridge_source / "scripts/public-draft-manifest.txt"

roots = {"clipward": bridge_source}
scanned_dirs = {
    "clipward": (".github", "bridge", "build", "docs", "engine", "resources", "scripts", "src", "tests"),
}
# Older planning and deployment notes are intentionally outside the public
# snapshot. Any other addition to a scanned directory needs manifest review.
excluded = {
    "clipward": {"docs/CLIPPING_REDESIGN_PLAN.md", "src/main/callback-server.ts"},
}
generated_dirs = {"__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache", ".venv"}
generated_files = {".DS_Store"}
blocked_names = {".npmrc", ".pypirc", "id_rsa", "id_ed25519"}
blocked_suffixes = {".pem", ".p8", ".p12", ".key", ".db", ".sqlite", ".sqlite3",
                    ".dump", ".dmp", ".har", ".token", ".secret", ".credentials", ".log"}


def fail(message: str) -> None:
    raise SystemExit(f"Public draft export refused: {message}")


if manifest_path.parent.is_symlink() or manifest_path.is_symlink() or not manifest_path.is_file():
    fail("the review manifest is missing or is a symlink")

allowed = {name: set() for name in roots}
for line in manifest_path.read_text(encoding="utf-8").splitlines():
    if not line or line.startswith("#"):
        continue
    name, separator, relative = line.partition("/")
    path = Path(relative)
    if (not separator or name not in roots or not relative or path.is_absolute()
            or any(part in (".", "..") for part in path.parts)):
        fail("the review manifest has an invalid entry")
    if (any(part.startswith(".env") or part == ".git" or part in blocked_names for part in path.parts)
            or path.suffix.lower() in blocked_suffixes):
        fail("the review manifest includes a blocked file type")
    if relative in allowed[name]:
        fail("the review manifest has a duplicate entry")
    allowed[name].add(relative)

for name, source in roots.items():
    if not allowed[name]:
        fail("the review manifest has no entries for the source repository")
    for relative in allowed[name]:
        current = source
        for part in Path(relative).parts:
            current = current / part
            if current.is_symlink():
                fail("a reviewed source path contains a symlink")
        if not current.is_file() or not stat.S_ISREG(current.stat().st_mode):
            fail("a reviewed source file is missing or not regular")

    for directory in scanned_dirs[name]:
        root = source / directory
        if root.is_symlink() or not root.is_dir():
            fail("a reviewed source directory is missing or is a symlink")
        for current, directories, files in os.walk(root, followlinks=False):
            for child in directories[:]:
                item = Path(current) / child
                if item.is_symlink():
                    fail("a scanned source directory contains a symlink")
                if child in generated_dirs:
                    directories.remove(child)
            for child in files:
                item = Path(current) / child
                relative = item.relative_to(source).as_posix()
                if item.is_symlink():
                    fail("a scanned source directory contains a symlink")
                if child in generated_files or child.endswith(".pyc"):
                    continue
                if relative not in allowed[name] and relative not in excluded[name]:
                    fail("a scanned source directory contains an unreviewed file")

parent = destination.parent.resolve(strict=True)
if destination.exists() or destination.is_symlink():
    fail("choose a new destination path")
if any(parent == source or source in parent.parents for source in roots.values()):
    fail("choose a destination outside the Clipward source repository")
if parent == Path("/"):
    fail("choose a dedicated parent directory for the draft")

destination.mkdir()
for name, source in roots.items():
    target = destination / name
    target.mkdir()
    for relative in sorted(allowed[name]):
        output = target / relative
        output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source / relative, output, follow_symlinks=False)

print(f"Reviewable Clipward source draft created at {destination}.")
print("Review ownership, dependency rights, and secret-scan results before publishing.")
PY
