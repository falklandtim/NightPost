#!/usr/bin/env python3
"""Write flatpak/cargo-sources.json from Cargo.lock.

flatpak-builder builds without the network, so it downloads every crate
first, as a source listed in the manifest. This lists each crate in
Cargo.lock, with the checksum Cargo.lock already records, unpacked where
cargo expects vendored crates, plus the cargo settings that point it there.

Run it again whenever Cargo.lock changes (the monthly "Update dependencies"
step in AGENTS.md section 6), and commit the result:

    scripts/flatpak-cargo-sources.py

Only Python's standard library, so it needs nothing installed. It does the
same job as flatpak-builder-tools' cargo generator, for the crates.io-only
dependencies NightPost has: a git dependency is refused rather than
guessed at.
"""
import json
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
VENDOR = "cargo/vendor"


def sources(lock: dict) -> list:
    out = []
    for pkg in lock.get("package", []):
        source = pkg.get("source")
        if source is None:
            continue  # NightPost's own crates
        if source != CRATES_IO:
            sys.exit(f"{pkg['name']} {pkg['version']} comes from {source}; only crates.io is handled here")
        name, version, checksum = pkg["name"], pkg["version"], pkg["checksum"]
        dest = f"{VENDOR}/{name}-{version}"
        out.append({
            "type": "archive",
            "archive-type": "tar-gzip",
            "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
            "sha256": checksum,
            "dest": dest,
        })
        # cargo checks vendored crates against this; an empty file list
        # means "trust the package checksum".
        out.append({
            "type": "inline",
            "contents": json.dumps({"package": checksum, "files": {}}),
            "dest": dest,
            "dest-filename": ".cargo-checksum.json",
        })
    out.append({
        "type": "inline",
        "contents": f'[source.vendored-sources]\ndirectory = "{VENDOR}"\n\n[source.crates-io]\nreplace-with = "vendored-sources"\n',
        "dest": "cargo",
        "dest-filename": "config.toml",
    })
    return out


def main() -> None:
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    result = sources(lock)
    path = ROOT / "flatpak" / "cargo-sources.json"
    path.parent.mkdir(exist_ok=True)
    path.write_text(json.dumps(result, indent=1) + "\n")
    crates = sum(1 for s in result if s["type"] == "archive")
    print(f"Wrote {path.relative_to(ROOT)}: {crates} crates")


if __name__ == "__main__":
    main()
