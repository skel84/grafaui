#!/usr/bin/env python3
"""Verify expanded fixtures; download missing snapshots from their recorded sources."""

import argparse
import hashlib
import json
from pathlib import Path
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Verify local snapshots without network access")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1] / "fixtures"
    manifest = json.loads((root / "sources/expanded.json").read_text())
    failures = []
    for source in manifest["dashboards"]:
        path = (root / source["file"]).resolve()
        if not path.is_relative_to(root):
            failures.append(f"Invalid fixture path: {source['file']}")
            continue
        try:
            exists = path.exists()
            if exists:
                data = path.read_bytes()
            elif args.check:
                raise ValueError("missing local snapshot")
            else:
                request = urllib.request.Request(source["source_url"], headers={"User-Agent": "grafaui-fixtures"})
                with urllib.request.urlopen(request, timeout=30) as response:
                    data = response.read()
            if hashlib.sha256(data).hexdigest() != source["sha256"]:
                raise ValueError("SHA-256 mismatch; snapshot has changed")
            dashboard = json.loads(data)
            dashboard = dashboard.get("dashboard", dashboard)
            if not isinstance(dashboard.get("panels"), list):
                raise ValueError("not a dashboard export")
            if not exists:
                path.parent.mkdir(parents=True, exist_ok=True)
                # Existing files are never overwritten, including concurrent writes.
                with path.open("xb") as target:
                    target.write(data)
        except (OSError, ValueError) as error:
            failures.append(f"{source['file']}: {error}")
    if failures:
        parser.exit(1, "\n".join(failures) + "\n")
    print(f"Verified {len(manifest['dashboards'])} dashboard snapshots.")


if __name__ == "__main__":
    main()
