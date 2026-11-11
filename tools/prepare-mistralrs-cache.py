#!/usr/bin/env python3
"""Repair the pinned SDK's missing build-script Git metadata watch path.

Run after fetching the runtime's locked dependencies. This changes only an
ignored symlink in Cargo's Git checkout; no SDK source files are modified.
"""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

REVISION = "fb4cdc3730dc64b366724e7637a205a8b7747499"
SOURCE = f"git+https://github.com/EricLBuehler/mistral.rs?rev={REVISION}#{REVISION}"
BUILD_SCRIPT_SHA256 = "d60cd38d65fc97f479a4d04bcca6db37973c940171c8423d9fb773af71f1fcd6"


def output(*command):
    return subprocess.check_output(command, text=True).strip()


def prepare(package):
    if package["source"] != SOURCE:
        raise ValueError("SDK source is not the expected pinned Git dependency")
    core = Path(package["manifest_path"]).resolve().parent
    checkout = core.parent
    if core.name != "mistralrs-core":
        raise ValueError("unexpected SDK checkout layout")
    if output("git", "-C", str(checkout), "rev-parse", "HEAD") != REVISION:
        raise ValueError("SDK checkout revision mismatch")
    if output("git", "-C", str(checkout), "status", "--porcelain", "--untracked-files=no"):
        raise ValueError("SDK checkout contains modified tracked files")
    if hashlib.sha256((core / "build.rs").read_bytes()).hexdigest() != BUILD_SCRIPT_SHA256:
        raise ValueError("SDK build script differs from the pinned version")
    metadata = checkout / ".git"
    if not metadata.is_dir() or not (metadata / "HEAD").is_file():
        raise ValueError("unsupported Git metadata layout; no files changed")
    link = core / ".git"
    if link.is_symlink():
        if link.resolve() != metadata.resolve():
            raise ValueError("existing SDK metadata link has a different target")
        action = "already prepared"
    elif link.exists():
        raise ValueError("SDK metadata path already exists; refusing to overwrite")
    else:
        link.symlink_to("../.git", target_is_directory=True)
        action = "prepared"
    if output("git", "-C", str(core), "rev-parse", "HEAD") != REVISION:
        raise ValueError("SDK revision check failed after preparing metadata")
    print(json.dumps({"revision": REVISION, "metadata_link": str(link), "action": action}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest-path", type=Path, default=Path(__file__).resolve().parents[1] / "products/runtime/Cargo.toml")
    parser.add_argument("--target", help="target triple (defaults to rustc's host)")
    args = parser.parse_args()
    target = args.target
    if target is None:
        target = next(line.removeprefix("host: ") for line in output("rustc", "-vV").splitlines() if line.startswith("host: "))
    metadata = json.loads(output("cargo", "metadata", "--locked", "--offline", "--filter-platform", target, "--format-version", "1", "--manifest-path", str(args.manifest_path)))
    packages = [package for package in metadata["packages"] if package["name"] == "mistralrs-core"]
    if len(packages) != 1:
        raise ValueError("expected exactly one mistralrs-core dependency")
    prepare(packages[0])


if __name__ == "__main__":
    main()
