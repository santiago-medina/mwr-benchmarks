#!/usr/bin/env python3
"""Materialize the pinned, vendored Servo fork with our in-process interpreter."""
import argparse
import hashlib
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request

REVISION = "aac43a3f31a259f04a574f5ec4e959c943ad7cc7"
SHA256 = "6e26f21e6c8bcb8ee3c374965fcaf11c05e07d7e37022143c120ed24b8a8b6a6"
URL = "https://github.com/servo/servo/releases/download/v0.7.0/servo-v0.7.0-src-vendored.tar.gz"
HERE = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--source", type=Path, default=HERE.parent / ".servo-native")
    parser.add_argument("--archive", type=Path)
    args = parser.parse_args()
    source = args.source.resolve()
    if source.exists():
        raise SystemExit(f"Refusing to overwrite existing source: {source}")
    archive = args.archive or source.with_suffix(".tar.gz")
    archive.parent.mkdir(parents=True, exist_ok=True)
    if not archive.exists():
        with urllib.request.urlopen(URL, timeout=120) as response, archive.open("wb") as out:
            shutil.copyfileobj(response, out)
    digest = hashlib.file_digest(archive.open("rb"), "sha256").hexdigest()
    if digest != SHA256:
        raise SystemExit(f"Servo source digest mismatch: {digest}")
    source.mkdir()
    with tarfile.open(archive) as tar:
        tar.extractall(source, filter="data")
    if (source / "GIT_REVISION").read_text().strip() != REVISION:
        raise SystemExit("Servo source revision mismatch")
    # git apply silently skips paths when this tree is nested in another git
    # repository. GNU patch always resolves paths against this source directory.
    command = ["patch", "--batch", "--forward", "-p1", "-i", str(HERE / "servo.patch")]
    subprocess.run(command + ["--dry-run"], cwd=source, check=True)
    subprocess.run(command, cwd=source, check=True)
    shutil.copy2(HERE / "mwr.rs", source / "components/script/mwr.rs")
    lab = source / "mwr-gc-lab"
    lab.mkdir()
    shutil.copy2(HERE.parent / "Cargo.toml", lab / "Cargo.toml")
    shutil.copytree(HERE.parent / "src", lab / "src")
    print(f"Prepared Servo {REVISION} at {source}")


if __name__ == "__main__":
    main()
