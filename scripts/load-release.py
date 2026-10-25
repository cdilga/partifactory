#!/usr/bin/env python3
"""Import exact release OCI bytes into a caller-owned OCI registry, then optionally pull.

Unlike docker load/tag/push, this preserves the release's original manifest digest.
The caller owns registry lifecycle, network exposure and exact-resource cleanup.
"""
from __future__ import annotations

import argparse
import hashlib
import http.client
import json
from pathlib import Path
import re
import ssl
import subprocess
import sys
import tarfile
from urllib.parse import parse_qsl, quote, urlencode, urlsplit


def require(condition, message):
    if not condition:
        raise ValueError(message)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--registry", required=True, help="Caller-owned http(s)://host:port registry origin")
    parser.add_argument("--repository", required=True)
    parser.add_argument("--tag", default="release")
    parser.add_argument("--pull", action="store_true")
    parser.add_argument("--host", help="Optional Docker endpoint or context for pull")
    parser.add_argument("--ca-cert", type=Path)
    parser.add_argument("--output", type=Path, help="Write immutable image reference as well as stdout")
    args = parser.parse_args()
    parsed = urlsplit(args.registry)
    require(parsed.scheme in ("http", "https") and parsed.hostname and not parsed.username
            and parsed.path in ("", "/") and not parsed.query and not parsed.fragment, "registry must be an origin without credentials")
    require(re.fullmatch(r"[a-z0-9]+(?:[._-][a-z0-9]+)*(?:/[a-z0-9]+(?:[._-][a-z0-9]+)*)*", args.repository), "invalid repository")
    require(re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}", args.tag), "invalid tag")
    sums = {}
    for line in (args.assets / "SHA256SUMS").read_text().splitlines():
        digest, name = line.split("  ", 1)
        require(Path(name).name == name and name not in sums, "unsafe release filename")
        require(re.fullmatch(r"[a-f0-9]{64}", digest), "invalid release checksum")
        sums[name] = digest
        with (args.assets / name).open("rb") as source:
            require(hashlib.file_digest(source, "sha256").hexdigest() == digest, "release checksum mismatch")
    require({"RELEASE.json", "digest.txt", "partifactory-linux-amd64.oci.tar", "part-linux-amd64.tar.gz"} <= sums.keys(), "release assets incomplete")
    release = json.loads((args.assets / "RELEASE.json").read_text())
    require(release["image"] == (args.assets / "digest.txt").read_text().strip(), "release references disagree")
    image_digest = release["image"].rsplit("@", 1)[1]
    require(re.fullmatch(r"sha256:[a-f0-9]{64}", image_digest), "invalid image digest")
    prefix = "/v2/" + args.repository
    context = ssl.create_default_context(cafile=str(args.ca_cert) if args.ca_cert else None)

    def request(method, path, body=None, headers=None, expected=(200,)):
        connection = (http.client.HTTPSConnection(parsed.hostname, parsed.port, timeout=120, context=context)
                      if parsed.scheme == "https" else http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=120))
        try:
            connection.request(method, path, body=body, headers=headers or {})
            response = connection.getresponse()
            data, outgoing = response.read(1024 * 1024), dict((k.lower(), v) for k, v in response.getheaders())
            require(response.status in expected, f"registry {method} failed with status {response.status}")
            return response.status, outgoing, data
        finally:
            connection.close()

    def upload_blob(digest, content):
        status, _, _ = request("HEAD", prefix + "/blobs/" + digest, expected=(200, 404))
        if status == 200:
            return
        _, headers, _ = request("POST", prefix + "/blobs/uploads/", body=b"", expected=(202,))
        # Registry Location may advertise its container-internal hostname. Keep
        # requests on the explicitly selected origin and preserve signed query state.
        location = urlsplit(headers["location"])
        require(location.path.startswith(prefix + "/blobs/uploads/"), "unsafe upload location")
        query = parse_qsl(location.query, keep_blank_values=True)
        query.append(("digest", digest))
        _, uploaded, _ = request("PUT", location.path + "?" + urlencode(query), body=content,
                                  headers={"Content-Type": "application/octet-stream"}, expected=(201,))
        require(uploaded.get("docker-content-digest", digest) == digest, "registry changed blob digest")

    with tarfile.open(args.assets / "partifactory-linux-amd64.oci.tar") as archive:
        def blob(digest):
            require(re.fullmatch(r"sha256:[a-f0-9]{64}", digest), "unsupported OCI digest")
            member = archive.getmember("blobs/sha256/" + digest.split(":", 1)[1])
            require(member.isfile(), "OCI blob is not a regular file")
            content = archive.extractfile(member).read()
            require("sha256:" + hashlib.sha256(content).hexdigest() == digest, "OCI blob hash mismatch")
            return content

        visited = set()

        def manifest(digest):
            if digest in visited:
                return blob(digest)
            visited.add(digest)
            raw = blob(digest)
            document = json.loads(raw)
            if "manifests" in document:
                for child in document["manifests"]:
                    manifest(child["digest"])
                media_type = document.get("mediaType", "application/vnd.oci.image.index.v1+json")
            else:
                require("config" in document and "layers" in document, "unsupported OCI manifest")
                for descriptor in [document["config"]] + document["layers"]:
                    upload_blob(descriptor["digest"], blob(descriptor["digest"]))
                media_type = document.get("mediaType", "application/vnd.oci.image.manifest.v1+json")
            _, headers, _ = request("PUT", prefix + "/manifests/" + digest, body=raw,
                                    headers={"Content-Type": media_type}, expected=(201,))
            require(headers.get("docker-content-digest", digest) == digest, "registry changed manifest digest")
            return raw

        raw = manifest(image_digest)
        document = json.loads(raw)
        media_type = document.get("mediaType", "application/vnd.oci.image.index.v1+json" if "manifests" in document
                                   else "application/vnd.oci.image.manifest.v1+json")
        _, headers, _ = request("PUT", prefix + "/manifests/" + quote(args.tag), body=raw,
                                headers={"Content-Type": media_type}, expected=(201,))
        require(headers.get("docker-content-digest", image_digest) == image_digest, "registry changed tagged digest")
        _, _, served = request("GET", prefix + "/manifests/" + image_digest, headers={"Accept": media_type})
        require(hashlib.sha256(served).digest() == hashlib.sha256(raw).digest(), "registry served different release manifest")
    image = parsed.netloc + "/" + args.repository + "@" + image_digest
    if args.pull:
        engine = ["docker"]
        if args.host:
            engine += ["--host" if "://" in args.host else "--context", args.host]
        subprocess.run(engine + ["pull", "--platform", "linux/amd64", image], check=True, stdout=sys.stderr, stderr=sys.stderr)
        inspected = subprocess.run(engine + ["image", "inspect", image], check=True, capture_output=True)
        require(image in json.loads(inspected.stdout)[0]["RepoDigests"], "Docker did not retain the immutable reference")
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(image + "\n")
    print(image)


if __name__ == "__main__":
    main()
