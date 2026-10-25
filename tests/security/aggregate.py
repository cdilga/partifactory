#!/usr/bin/env python3
"""Merge actual CI execution evidence; perform no network requests or implicit test runs."""
from __future__ import annotations

import argparse
import ast
import hashlib
import json
from pathlib import Path
import sys

from coverage import CHECKLIST, ROOT, check_mapping, observations


def checklist_hash():
    return hashlib.sha256(CHECKLIST.read_bytes()).hexdigest()


def aggregate(checklist, reports, evidence, platform="all"):
    checks = []
    outside = []
    for row in checklist["checks"]:
        components = []
        for index, component in enumerate(row["components"]):
            kind = component["kind"]
            required_platform = component.get("platform")
            if required_platform and platform != "all" and required_platform != platform:
                result = {"outcome": "outside_platform", "platform": required_platform}
                outside.append({"id": row["id"], "proves": component["proves"], "platform": required_platform})
            elif kind in ("pending", "manual"):
                result = {"outcome": kind, "owner": component["owner"]}
            elif kind == "external":
                observed = []
                for report in reports:
                    for previous in report.get("checks", []):
                        if previous.get("id") != row["id"]:
                            continue
                        previous_components = previous.get("components", [])
                        if index < len(previous_components) and previous_components[index].get("proves") == component["proves"]:
                            observed.append(previous_components[index].get("outcome"))
                # A failed observation is never overwritten by a pass from another input.
                outcome = "fail" if "fail" in observed else "pass" if "pass" in observed else "unobserved"
                result = {"outcome": outcome, "probe": component["probe"]}
            else:
                key = (kind, component["file"], component["test"])
                seen = evidence.get(key, set())
                required = component.get("projects", [])
                passed = all(project in seen for project in required) if required else bool(seen)
                result = {"outcome": "pass" if passed else "unobserved", "file": component["file"], "test": component["test"]}
                if required:
                    result["required_projects"] = required
                    result["observed_projects"] = sorted(seen)
            components.append({"proves": component["proves"], **result})
        outcomes = {component["outcome"] for component in components}
        if "fail" in outcomes:
            outcome = "fail"
        elif "unobserved" in outcomes:
            outcome = "unobserved"
        elif "pass" in outcomes:
            outcome = "pass"
        elif "pending" in outcomes:
            outcome = "pending"
        elif "manual" in outcomes:
            outcome = "manual"
        else:
            outcome = "outside_platform"
        checks.append({"event": "sec.check", "id": row["id"], "outcome": outcome, "components": components})
    complete = not any(row["outcome"] in ("fail", "unobserved") for row in checks)
    return {"schema": 1, "platform": platform, "checklist_hash": checklist_hash(), "complete": complete,
            "outside_platform": outside, "checks": checks}


def renderer_observed(path):
    # The actual observer prints this only after every isolation and TLS assertion passes.
    # Read its source without executing it to bind the accepted line to the current observer.
    tree = ast.parse((ROOT / "tests/render/isolation.py").read_text())
    messages = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id == "print":
            messages.extend(a.value for a in node.args if isinstance(a, ast.Constant) and isinstance(a.value, str) and a.value.startswith("PASS render:"))
    lines = Path(path).read_text().splitlines()
    return any(message in lines for message in messages)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--external-report", action="append", default=[])
    parser.add_argument("--native-junit", action="append", default=[])
    parser.add_argument("--playwright-report", action="append", default=[])
    parser.add_argument("--script-evidence", action="append", default=[])
    parser.add_argument("--renderer-summary", action="append", default=[])
    parser.add_argument("--proxy-runtime-report", action="append", default=[])
    parser.add_argument("--artifact-root", type=Path, help="discover evidence downloaded from this workflow's jobs")
    parser.add_argument("--platform", choices=("all", "linux", "macos"), default="all")
    parser.add_argument("--require-complete", action="store_true")
    parser.add_argument("--evidence-dir", default="target/e2e-logs/security-aggregate")
    args = parser.parse_args()
    errors, checklist = check_mapping()
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    if args.artifact_root:
        for path in sorted(args.artifact_root.rglob("*")):
            if not path.is_file():
                continue
            if path.name == "junit.xml":
                args.native_junit.append(str(path))
            elif path.name == "summary.txt":
                args.renderer_summary.append(str(path))
            elif path.name == "proxy-runtime-log-gate.json":
                args.proxy_runtime_report.append(str(path))
            elif path.suffix == ".json":
                data = json.loads(path.read_text())
                if isinstance(data, dict) and data.get("schema") == 1 and "probe_results" in data:
                    args.external_report.append(str(path))
                elif isinstance(data, dict) and "suites" in data and "config" in data:
                    args.playwright_report.append(str(path))
    reports = []
    for path in args.external_report:
        report = json.loads(Path(path).read_text())
        if report.get("checklist_hash") != checklist_hash():
            print(f"external report does not match current checklist: {path}", file=sys.stderr)
            return 1
        reports.append(report)
    evidence = observations(args.native_junit, args.playwright_report, args.script_evidence)
    for path in args.renderer_summary:
        if renderer_observed(path):
            evidence[("script", "tests/render/isolation.py", "renderer_isolation")].add("script")
    for path in args.proxy_runtime_report:
        data = json.loads(Path(path).read_text())
        if data.get("event") == "sec.check" and data.get("outcome") == "pass":
            lanes = {row.get("lane"): row for row in data.get("observations", [])}
            if (lanes.get("control", {}).get("canaries_exposed", 0) > 0 and
                lanes.get("shipped", {}).get("canaries_exposed") == 0 and
                lanes["shipped"].get("runtime_warnings", 0) > 0):
                evidence[("script", "deploy/proxy/test-runtime-logs.py", "runtime_proxy_log_redaction")].add("script")
    report = aggregate(checklist, reports, evidence, args.platform)
    report["inputs"] = {"external_reports": args.external_report, "native_junit": args.native_junit,
                        "playwright_reports": args.playwright_report, "script_evidence": args.script_evidence,
                        "renderer_summaries": args.renderer_summary}
    report["inputs"]["proxy_runtime_reports"] = args.proxy_runtime_report
    destination = Path(args.evidence_dir)
    destination.mkdir(parents=True, exist_ok=True)
    for row in report["checks"]:
        folder = destination / row["id"]
        folder.mkdir(exist_ok=True)
        (folder / "result.json").write_text(json.dumps(row, indent=2) + "\n")
        print(json.dumps({key: row[key] for key in ("event", "id", "outcome")}))
    (destination / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"Security evidence platform={args.platform}, complete={report['complete']}, outside_platform={len(report['outside_platform'])}")
    return int(any(row["outcome"] == "fail" or (args.require_complete and row["outcome"] == "unobserved") for row in report["checks"]))


if __name__ == "__main__":
    raise SystemExit(main())
