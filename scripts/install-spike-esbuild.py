#!/usr/bin/env python3
"""Install the spike's exact esbuild binary after verifying its platform checksum."""
import argparse
import hashlib
import io
import os
from pathlib import Path
import platform
import tarfile
from urllib.request import urlopen

VERSION = "0.28.2"
PINS = {
    ("Darwin", "arm64"): ("darwin-arm64", "10b6243df618d374bb2d5c9cfbe7052e1405f6aa4e53a6164f11a91b9f2e1384"),
    ("Linux", "x86_64"): ("linux-x64", "e1698a3d5c6c0798fee4fd3b5cc816651f460c63d390a7a26ea4beb0b1884100"),
    ("Linux", "aarch64"): ("linux-arm64", "90bd269553d258e80b19e3ccab4f98f0831f87442a2f056510ce24fd1b425fc2"),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    key = (platform.system(), platform.machine())
    if key not in PINS:
        parser.error(f"no verified esbuild pin for {key}")
    package, expected = PINS[key]
    url = f"https://registry.npmjs.org/@esbuild/{package}/-/{package}-{VERSION}.tgz"
    with urlopen(url, timeout=60) as response:
        archive = response.read(32 * 1024 * 1024 + 1)
    if len(archive) > 32 * 1024 * 1024:
        raise ValueError("esbuild package exceeded the download limit")
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as tar:
        member = tar.getmember("package/bin/esbuild")
        if not member.isfile() or member.size > 32 * 1024 * 1024:
            raise ValueError("invalid esbuild binary entry")
        binary = tar.extractfile(member).read()
    if hashlib.sha256(binary).hexdigest() != expected:
        raise ValueError("esbuild binary checksum mismatch")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_name(args.output.name + f".tmp-{os.getpid()}")
    try:
        temporary.write_bytes(binary)
        temporary.chmod(0o755)
        temporary.replace(args.output)
    finally:
        temporary.unlink(missing_ok=True)
    print(f"esbuild {VERSION} {package} sha256:{expected}")


if __name__ == "__main__":
    main()
