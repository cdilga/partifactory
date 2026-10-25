#!/usr/bin/env python3
"""Package an already-exported OCI release without recompiling or changing its bytes."""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import re
import tarfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--image", required=True, help="Registry/repository, without tag or digest")
    parser.add_argument("--tag", required=True)
    parser.add_argument("--published", action="store_true")
    args = parser.parse_args()
    root = args.assets
    metadata = json.loads((root / "build.json").read_text())
    digest = metadata["containerimage.digest"]
    if not re.fullmatch(r"sha256:[a-f0-9]{64}", digest):
        raise SystemExit("invalid image digest")
    with tarfile.open(root / "partifactory-linux-amd64.oci.tar") as archive:
        def blob(value):
            if not re.fullmatch(r"sha256:[a-f0-9]{64}", value):
                raise SystemExit("unsupported OCI digest")
            member = archive.getmember("blobs/sha256/" + value.split(":", 1)[1])
            if not member.isfile():
                raise SystemExit("OCI blob must be a regular file")
            raw = archive.extractfile(member).read()
            if "sha256:" + hashlib.sha256(raw).hexdigest() != value:
                raise SystemExit("OCI blob checksum mismatch")
            return raw

        def manifest_for(value):
            document = json.loads(blob(value))
            if "layers" in document:
                return document
            candidates = [entry for entry in document.get("manifests", [])
                          if entry.get("platform", {}).get("architecture") == "amd64"
                          and entry.get("platform", {}).get("os") == "linux"]
            if len(candidates) != 1:
                raise SystemExit("OCI index must contain exactly one Linux amd64 image")
            return manifest_for(candidates[0]["digest"])

        manifest = manifest_for(digest)
        config = json.loads(blob(manifest["config"]["digest"]))
        if config.get("architecture") != "amd64" or config.get("os") != "linux":
            raise SystemExit("release is not Linux amd64")
        # Packaging may resume after the checkout advances. The image's own
        # verified config, rather than current HEAD, is the source of provenance.
        commit = config["config"].get("Labels", {}).get("org.opencontainers.image.revision", "")
        if not re.fullmatch(r"[a-f0-9]{40,64}", commit):
            raise SystemExit("release image has no source revision")
        binary = None
        for layer in manifest["layers"]:
            raw = blob(layer["digest"])
            with tarfile.open(fileobj=io.BytesIO(raw), mode="r:*") as contents:
                for member in contents:
                    if member.name.lstrip("./") == "partifactory" and member.isfile():
                        binary = contents.extractfile(member).read()
        if binary is None or binary[:4] != b"\x7fELF":
            raise SystemExit("image contains no ELF binary")
    (root / "digest.txt").write_text(f"{args.image}@{digest}\n")
    with (root / "part-linux-amd64.tar.gz").open("wb") as output:
        with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0) as zipped:
            with tarfile.open(fileobj=zipped, mode="w") as asset:
                member = tarfile.TarInfo("part")
                member.mode, member.size, member.mtime = 0o755, len(binary), 0
                asset.addfile(member, io.BytesIO(binary))
    (root / "RELEASE.json").write_text(json.dumps({"tag": args.tag, "image": f"{args.image}@{digest}",
                                                   "published": args.published, "commit": commit,
                                                   "config": manifest["config"]["digest"]}, indent=2) + "\n")
    files = ["RELEASE.json", "digest.txt", "part-linux-amd64.tar.gz", "partifactory-linux-amd64.oci.tar"]
    checksums = []
    for name in sorted(files):
        with (root / name).open("rb") as content:
            checksums.append(f'{hashlib.file_digest(content, "sha256").hexdigest()}  {name}\n')
    (root / "SHA256SUMS").write_text("".join(checksums))
    print(f"Release assets: {root}; image {digest}; source {commit}; published={args.published}")


if __name__ == "__main__":
    main()
