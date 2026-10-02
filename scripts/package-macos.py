#!/usr/bin/env python3
"""Package an already-built native macOS binary with its local dashboards."""

import argparse
import hashlib
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ARCHITECTURES = {
    "aarch64-apple-darwin": "arm64",
    "x86_64-apple-darwin": "x86_64",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=ARCHITECTURES)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    binary = root / "target" / args.target / "release/grafaui"
    subprocess.run(["lipo", str(binary), "-verify_arch", ARCHITECTURES[args.target]], check=True)
    subprocess.run([str(binary), "--version"], check=True)
    distribution = root / "dist"
    distribution.mkdir(exist_ok=True)
    name = f"grafaui-{args.target}"
    archive = distribution / f"{name}.tar.gz"
    with tempfile.TemporaryDirectory(prefix="grafaui-package-", dir=root / "target") as temporary:
        package = Path(temporary) / name
        package.mkdir()
        shutil.copy2(binary, package / "grafaui")
        for filename in ("README.md", "LICENSE"):
            shutil.copy2(root / filename, package / filename)
        shutil.copytree(root / "fixtures", package / "fixtures")
        # Smoke-test the packaged layout independently of the build directory.
        subprocess.run([str(package / "grafaui"), "--help"], check=True, stdout=subprocess.DEVNULL)
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(package, arcname=name)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(".gz.sha256").write_text(f"{digest}  {archive.name}\n")
    print(f"Created {archive.name} and SHA-256 checksum")


if __name__ == "__main__":
    main()
