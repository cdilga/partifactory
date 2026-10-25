#!/usr/bin/env python3
"""Restore release assets on an empty, independent OCI daemon, without remote bind mounts."""
from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import secrets
import subprocess
import tarfile
import tempfile
import time
import tomllib

REPO = Path(__file__).resolve().parents[1]


class Failed(Exception):
    pass


def require(condition, stage):
    if not condition:
        raise Failed(stage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", required=True, help="Docker context name or Docker endpoint")
    parser.add_argument("--source-daemon-id", help="Source Docker info ID when the source is offline")
    for flag in ("assets", "export", "site", "secrets-dir", "tls-cert", "tls-key", "ca-cert", "report-dir"):
        parser.add_argument("--" + flag, type=Path, required=True)
    parser.add_argument("--identity-file", type=Path, action="append", default=[])
    parser.add_argument("--https-port", type=int, default=19443)
    parser.add_argument("--address", default="127.0.0.1", help="Published target proxy address, including outer dind mapping")
    parser.add_argument("--e2e-command", nargs=argparse.REMAINDER, required=True,
                        help="External test argv; receives PF_BASE_URL, PF_CA, PF_CONNECT_TO and PF_DRILL_EVIDENCE")
    args = parser.parse_args()
    require(args.e2e_command, "external_e2e_command_required")
    start, at = time.monotonic(), time.time_ns() // 1_000_000
    failures, volumes, containers, created_images = [], [], [], []
    run_id = "pf-drill-" + secrets.token_hex(6)
    args.report_dir.mkdir(parents=True, exist_ok=True)
    evidence = args.report_dir / (str(at) + "-evidence")
    evidence.mkdir(mode=0o700)
    private_log = evidence / "private.log"
    private_log.touch(mode=0o600)
    environment = dict(os.environ)
    for name in ("DOCKER_HOST", "DOCKER_CONTEXT", "COMPOSE_FILE", "COMPOSE_PROFILES", "COMPOSE_PROJECT_NAME"):
        environment.pop(name, None)
    target = ["docker", "--host" if "://" in args.host else "--context", args.host]
    compose = None
    stage = "preflight"

    def run(argv, *, env=environment, data=None, timeout=120, check=True):
        remaining = 900 - (time.monotonic() - start)
        require(remaining > 0, "recovery_target_exceeded")
        result = subprocess.run(argv, input=data, capture_output=True, env=env,
                                timeout=min(timeout, remaining), cwd=REPO)
        with private_log.open("ab") as log:
            log.write(result.stdout + result.stderr)
        if check:
            require(result.returncode == 0, stage)
        return result

    def docker(*argv, **kwargs):
        return run(target + list(argv), **kwargs)

    def snapshot_images():
        return set(docker("image", "ls", "-aq", "--no-trunc").stdout.decode().split())

    def stage_volume(name, entries, uid=65532):
        volume = run_id + "-" + name
        docker("volume", "create", "--label", "pf.harness=1", "--label", "pf.drill=" + run_id, volume)
        volumes.append(volume)
        container = docker("create", "--label", "pf.harness=1", "--label", "pf.drill=" + run_id,
                           "--user", "0:0", "--mount", f"type=volume,src={volume},dst=/stage", image).stdout.decode().strip()
        containers.append(container)
        # docker cp accepts a tar stream and does not need a shell or a writable image.
        with tempfile.TemporaryFile() as stream:
            with tarfile.open(fileobj=stream, mode="w") as archive:
                root = tarfile.TarInfo(".")
                root.type, root.mode, root.uid, root.gid = tarfile.DIRTYPE, 0o700, uid, uid
                archive.addfile(root)
                for path, source, mode in entries:
                    member = tarfile.TarInfo(path)
                    member.size, member.mode, member.uid, member.gid = source.stat().st_size, mode, uid, uid
                    with source.open("rb") as content:
                        archive.addfile(member, content)
            stream.seek(0)
            remaining = max(1, 900 - (time.monotonic() - start))
            result = subprocess.run(target + ["cp", "-a", "-", container + ":/stage"], stdin=stream,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=environment, timeout=remaining)
            with private_log.open("ab") as log:
                log.write(result.stdout + result.stderr)
            require(result.returncode == 0, "transfer_offline_files")
        docker("rm", container)
        containers.remove(container)
        return volume

    try:
        for path in [args.export, args.site, args.tls_cert, args.tls_key, args.ca_cert] + args.identity_file:
            require(path.is_file(), "offline_file_missing")
        config = tomllib.loads(args.site.read_text())
        require(not config.get("mirror", {}).get("test_resolve"), "dev_config_not_allowed")
        apex = config["domain"]["apex"]
        workspace = config["domain"].get("workspace_host", "parts")
        if "." not in workspace:
            workspace += "." + apex
        source_id = args.source_daemon_id or run(["docker", "info", "--format", "{{.ID}}"], timeout=20).stdout.decode().strip()
        target_id = docker("info", "--format", "{{.ID}}", timeout=20).stdout.decode().strip()
        require(source_id and target_id and source_id != target_id, "second_daemon_required")
        require(not docker("ps", "-aq").stdout.strip() and not snapshot_images()
                and not docker("volume", "ls", "-q").stdout.strip(), "target_daemon_not_fresh")
        (evidence / "daemon.json").write_text(json.dumps({"source_id": source_id, "target_id": target_id}) + "\n")

        stage = "release_checksums"
        checksums = {}
        for line in (args.assets / "SHA256SUMS").read_text().splitlines():
            digest, name = line.split("  ", 1)
            require(Path(name).name == name and name not in checksums, "unsafe_release_filename")
            checksums[name] = digest
            with (args.assets / name).open("rb") as source:
                require(hashlib.file_digest(source, "sha256").hexdigest() == digest, "release_checksum_mismatch")
        require({"RELEASE.json", "partifactory-linux-amd64.oci.tar", "part-linux-amd64.tar.gz", "digest.txt"} <= checksums.keys(), "release_assets_incomplete")
        release = json.loads((args.assets / "RELEASE.json").read_text())
        require(release["image"] == (args.assets / "digest.txt").read_text().strip(), "release_digest_mismatch")
        digest = release["image"].rsplit("@", 1)[1]
        with tempfile.TemporaryDirectory(prefix=run_id + "-", dir=evidence) as temp:
            temp = Path(temp)
            # Docker's classic image store cannot load all OCI archives. Convert verified
            # OCI blobs to Docker's archive format without changing the image config.
            stage = "load_release_image"
            with tarfile.open(args.assets / "partifactory-linux-amd64.oci.tar") as oci:
                def blob(value):
                    raw = oci.extractfile("blobs/sha256/" + value.split(":", 1)[1]).read()
                    require("sha256:" + hashlib.sha256(raw).hexdigest() == value, "oci_checksum_mismatch")
                    return raw
                manifest = json.loads(blob(digest))
                while "layers" not in manifest:
                    candidates = [x for x in manifest.get("manifests", []) if x.get("platform", {}).get("architecture") == "amd64" and x.get("platform", {}).get("os") == "linux"]
                    require(len(candidates) == 1, "release_platform_ambiguous")
                    manifest = json.loads(blob(candidates[0]["digest"]))
                config_raw = blob(manifest["config"]["digest"])
                image = manifest["config"]["digest"]
                image_config = json.loads(config_raw)
                require(str(image_config["config"].get("User")) in ("65532", "65532:65532"), "release_user_not_nonroot")
                layers, image_binary = [], None
                for i, descriptor in enumerate(manifest["layers"]):
                    raw = blob(descriptor["digest"])
                    if raw[:2] == b"\x1f\x8b":
                        raw = gzip.decompress(raw)
                    require("sha256:" + hashlib.sha256(raw).hexdigest() == image_config["rootfs"]["diff_ids"][i], "oci_layer_mismatch")
                    layer_name = f"layer-{i}.tar"
                    (temp / layer_name).write_bytes(raw)
                    layers.append(layer_name)
                    with tarfile.open(fileobj=io.BytesIO(raw)) as contents:
                        for member in contents:
                            if member.name.lstrip("./") == "partifactory" and member.isfile():
                                image_binary = contents.extractfile(member).read()
                with tarfile.open(args.assets / "part-linux-amd64.tar.gz") as cli:
                    binary = cli.extractfile("part").read()
                require(binary == image_binary, "release_cli_image_mismatch")
                (temp / "part").write_bytes(binary)
                (temp / "config.json").write_bytes(config_raw)
                (temp / "manifest.json").write_text(json.dumps([{"Config": "config.json", "RepoTags": [run_id + ":release"], "Layers": layers}]))
                archive_path = temp / "docker.tar"
                with tarfile.open(archive_path, "w") as archive:
                    for name in ["config.json", "manifest.json"] + layers:
                        archive.add(temp / name, arcname=name)
            docker("load", "--input", str(archive_path), timeout=300)
            created_images.append(image)
            require(docker("image", "inspect", "--format", "{{.Id}}", image).stdout.decode().strip() == image, "loaded_image_mismatch")

            stage = "transfer_offline_files"
            data_volume = stage_volume("data", [])
            secret_names = ["session_key", "cap_hmac_key", "metrics_token", "idp_client_secret"]
            secret_volume = stage_volume("secrets", [(name, args.secrets_dir / name, 0o600) for name in secret_names] + [("site.toml", args.site, 0o600)])
            restore_entries = [("archive", args.export, 0o600), ("part", temp / "part", 0o755)]
            restore_entries += [(f"identity-{i}", path, 0o600) for i, path in enumerate(args.identity_file)]
            restore_volume = stage_volume("restore", restore_entries)
            scratch_volume = stage_volume("scratch", [])
            proxy_volume = stage_volume("proxy", [("Caddyfile", REPO / "deploy/proxy/Caddyfile", 0o644),
                                                    ("maintenance.html", REPO / "deploy/proxy/maintenance.html", 0o644),
                                                    ("cert.pem", args.tls_cert, 0o644), ("key.pem", args.tls_key, 0o600)], uid=0)
            compose_env = dict(environment, PF_IMAGE=release["image"], PF_DOMAIN_APEX=apex,
                               PF_DOMAIN_WORKSPACE_HOST=workspace.removesuffix("." + apex),
                               PF_SITE_CONFIG=str(args.site.resolve()), PF_TLS_CERT=str(args.tls_cert.resolve()), PF_TLS_KEY=str(args.tls_key.resolve()))
            for name in secret_names:
                compose_env["PF_" + name.upper() + "_FILE"] = str((args.secrets_dir / name).resolve())
            normalized = json.loads(run(["docker", "compose", "-f", str(REPO / "deploy/compose.yaml"), "--profile", "proxy", "config", "--format", "json"], env=compose_env).stdout)
            normalized["name"] = run_id
            normalized["services"] = {name: value for name, value in normalized["services"].items() if name in ("partifactory", "proxy")}
            normalized.pop("secrets", None)
            normalized["volumes"] = {name: {"external": True, "name": name} for name in volumes}
            for network in normalized["networks"].values():
                network.pop("name", None)
                network["labels"] = {"pf.harness": "1", "pf.drill": run_id}
            for service in normalized["services"].values():
                service["labels"] = {"pf.harness": "1", "pf.drill": run_id}
                service["restart"] = "no"
                service.pop("secrets", None)
            app = normalized["services"]["partifactory"]
            app["image"], app["ports"] = image, []
            app["environment"]["PF_RENDER_ENABLED"] = "false"
            app["environment"]["PF_SITE_TOML"] = "/run/secrets/site.toml"
            app["environment"].update(PF_LISTEN_WORKSPACE="0.0.0.0:8080", PF_LISTEN_CONTENT="0.0.0.0:8081", PF_LISTEN_OPS="0.0.0.0:9090")
            # Archive verification stages every entry before touching /data. A
            # normal 64MiB runtime tmpfs must not cap the size of a recovery.
            app["tmpfs"] = []
            app["volumes"] = [{"type": "volume", "source": source, "target": dest, "read_only": readonly} for source, dest, readonly in
                              [(data_volume, "/data", False), (secret_volume, "/run/secrets", True), (restore_volume, "/restore", True), (scratch_volume, "/tmp", False)]]
            proxy = normalized["services"]["proxy"]
            proxy["volumes"] = [{"type": "volume", "source": proxy_volume, "target": "/etc/pf-drill", "read_only": True}]
            proxy["command"] = ["caddy", "run", "--config", "/etc/pf-drill/Caddyfile", "--adapter", "caddyfile"]
            proxy["environment"].update(PF_TLS_CERT="/etc/pf-drill/cert.pem", PF_TLS_KEY="/etc/pf-drill/key.pem")
            # Keep the delivered maintenance route at its delivered path too.
            proxy["volumes"].append({"type": "volume", "source": proxy_volume, "target": "/srv/pf-proxy", "read_only": True})
            proxy["ports"] = [{"target": 443, "published": str(args.https_port), "host_ip": "0.0.0.0", "protocol": "tcp"}]
            compose_path = evidence / "compose.json"
            compose_path.write_text(json.dumps(normalized))
            compose = target + ["compose", "-f", str(compose_path), "-p", run_id, "--profile", "proxy"]
            stage = "release_cli_import"
            imported = run(compose + ["run", "--rm", "--no-deps", "--entrypoint", "/restore/part", "partifactory", "import", "/restore/archive"] +
                           [value for i in range(len(args.identity_file)) for value in ("--identity", f"/restore/identity-{i}")], timeout=400)
            require(b"import.boot_epoch_bump" in imported.stdout + imported.stderr, "import_epoch_log_missing")
            stage = "compose_start"
            created_images.append(proxy["image"])
            run(compose + ["up", "-d"], timeout=300)
            stage = "restored_readiness"
            while True:
                result = run(compose + ["exec", "-T", "partifactory", "/partifactory", "healthcheck"], timeout=10, check=False)
                if result.returncode == 0:
                    break
                require(time.monotonic() - start < 850, "restored_readiness_timeout")
                time.sleep(1)
            logs = run(compose + ["logs", "--no-color"]).stdout
            (evidence / "origin.log").write_bytes(logs)
            stage = "external_e2e_failed"
            test_env = dict(environment, PF_BASE_URL="https://" + workspace, PF_CA=str(args.ca_cert.resolve()),
                            PF_CONNECT_TO=f"{args.address}:{args.https_port}", PF_DRILL_EVIDENCE=str(evidence.resolve()),
                            PF_DRILL_IMAGE=release["image"], PF_DRILL_COMPOSE=str(compose_path.resolve()),
                            PF_DRILL_HOST=args.host)
            run(args.e2e_command, env=test_env, timeout=900)
            (evidence / "release.json").write_text(json.dumps(release) + "\n")
            stage = "cleanup"
            run(compose + ["down", "-v", "--remove-orphans"], timeout=90)
            compose = None
    except KeyboardInterrupt:
        failures.append("interrupted")
    except Exception as error:
        failures.append(str(error) if isinstance(error, Failed) else stage)
    finally:
        # Cleanup uses its own timeout after the recovery timer expires. Never prune.
        cleanup_commands = []
        if compose:
            cleanup_commands.append(compose + ["down", "-v", "--remove-orphans"])
        cleanup_commands += [target + ["rm", "-f", container] for container in containers]
        cleanup_commands += [target + ["volume", "rm", volume] for volume in reversed(volumes)]
        for image_id in dict.fromkeys(reversed(created_images)):
            try:
                inspected = subprocess.run(target + ["image", "inspect", image_id], capture_output=True, env=environment, timeout=20)
                if inspected.returncode == 0:
                    cleanup_commands.append(target + ["image", "rm", image_id])
            except (OSError, subprocess.TimeoutExpired):
                failures.append("cleanup_failed")
        for command in cleanup_commands:
            try:
                result = subprocess.run(command, capture_output=True, env=environment, timeout=90)
                with private_log.open("ab") as log:
                    log.write(result.stdout + result.stderr)
                if result.returncode:
                    failures.append("cleanup_failed")
            except (OSError, subprocess.TimeoutExpired):
                failures.append("cleanup_failed")
        elapsed = time.monotonic() - start
        if elapsed >= 900:
            failures.append("recovery_target_exceeded")
        report = {"at": at, "ok": not failures, "rto_s": round(elapsed, 3), "failures": list(dict.fromkeys(failures))}
        destination = args.report_dir / f"{at}.json"
        temporary = destination.with_suffix(".tmp")
        with temporary.open("w") as output:
            json.dump(report, output)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        temporary.replace(destination)
        print(json.dumps(report))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
