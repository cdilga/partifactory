#!/usr/bin/env python3
"""Validate the §7 inventory separately from evidence that its tests ran."""
from __future__ import annotations

import argparse
from collections import defaultdict
import json
from pathlib import Path
import re
import sys
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
CHECKLIST = Path(__file__).with_name("checklist.yaml")


def plan_lines(text):
    section = text.split("## 7. Security checklist", 1)[1].split("\n## 8.", 1)[0]
    lines = []
    for line in section.splitlines():
        if line.startswith("- "):
            lines.append(line[2:].strip())
        elif line.startswith("  ") and lines:
            lines[-1] += " " + line.strip()
    return lines


def check_mapping(root=ROOT, checklist_path=CHECKLIST):
    errors = []
    checklist = json.loads(Path(checklist_path).read_text())  # JSON is YAML 1.2; no third-party parser.
    requirements = plan_lines((root / "PLAN.md").read_text())
    checks = checklist.get("checks", [])
    if len(requirements) != len(checks):
        errors.append(f"PLAN §7 has {len(requirements)} lines; checklist maps {len(checks)}")
    ids = set()
    runner_source = (root / "tests/security/run.py").read_text()
    for i, check in enumerate(checks):
        identifier = check.get("id", "")
        if not re.fullmatch(r"SEC-\d{3}", identifier) or identifier in ids:
            errors.append(f"invalid or duplicate id {identifier}")
        ids.add(identifier)
        if i >= len(requirements) or check.get("requirement") != requirements[i]:
            errors.append(f"{identifier}: requirement changed or not mapped in PLAN order")
        if not check.get("components"):
            errors.append(f"{identifier}: no coverage components")
        for component in check.get("components", []):
            kind = component.get("kind")
            if not component.get("proves"):
                errors.append(f"{identifier}: component lacks a precise scope")
            if kind in ("pending", "manual"):
                if not re.fullmatch(r"pf-[a-z0-9]+", component.get("owner", "")):
                    errors.append(f"{identifier}: {kind} requires owning bead")
                if kind == "manual" and (identifier != "SEC-050" or component["owner"] != "pf-vl9"):
                    errors.append(f"{identifier}: only real devices may be manual (pf-vl9)")
            elif kind == "external":
                probe = component.get("probe", "")
                if not probe or not re.search(r"^    def " + re.escape(probe) + r"\(", runner_source, re.M):
                    errors.append(f"{identifier}: external probe does not exist: {probe}")
            elif kind in ("native", "playwright", "script"):
                path = root / component.get("file", "")
                if not path.is_file():
                    errors.append(f"{identifier}: reference file missing: {component.get('file')}")
                    continue
                source = path.read_text()
                symbol = component.get("test", "")
                if kind == "native":
                    exists = bool(symbol) and re.search(r"\bfn\s+" + re.escape(symbol) + r"\s*\(", source)
                elif kind == "playwright":
                    exists = symbol and ("test('" + symbol + "'" in source or 'test("' + symbol + '"' in source)
                    if not component.get("projects"):
                        errors.append(f"{identifier}: browser references require explicit observed projects")
                else:
                    exists = component.get("source_anchor", "") and component["source_anchor"] in source
                if not exists:
                    errors.append(f"{identifier}: referenced {kind} test missing: {symbol}")
            else:
                errors.append(f"{identifier}: existing feature has no executable coverage: {kind}")
    return errors, checklist


def observations(junit_paths=(), playwright_paths=(), script_paths=()):
    """Only executed successes count. Skipped/expected-failed/flaky results cannot satisfy gates."""
    evidence = defaultdict(set)
    _, checklist = check_mapping()
    references = [c for row in checklist["checks"] for c in row["components"]
                  if c["kind"] in ("native", "playwright", "script")]
    for path in junit_paths:
        tree = ET.parse(path)
        for case in tree.iter("testcase"):
            if any(case.find(tag) is not None for tag in ("skipped", "failure", "error")):
                continue
            name = case.get("name", "")
            classname = case.get("classname", "")
            for ref in references:
                if ref["kind"] != "native":
                    continue
                symbol = ref["test"]
                if name == symbol or name.endswith("::" + symbol):
                    # A source file in reports is preferred; crate-qualified names also disambiguate.
                    crate = Path(ref["file"]).parts[1]
                    aliases = (crate, crate.replace("-", "_"))
                    qualified = classname.split("::", 1)[0] in aliases or name.split("::", 1)[0] in aliases
                    if ref.get("junit_classname"):
                        qualified = classname == ref["junit_classname"]
                    if qualified or case.get("file") == ref["file"]:
                        evidence[("native", ref["file"], symbol)].add("native")
    for path in playwright_paths:
        data = json.loads(Path(path).read_text())

        def visit(suites):
            for suite in suites:
                for spec in suite.get("specs", []):
                    for test in spec.get("tests", []):
                        results = test.get("results", [])
                        if test.get("expectedStatus") != "passed" or test.get("status") != "expected" or not results:
                            continue
                        if any(r.get("status") != "passed" for r in results):
                            continue
                        for ref in references:
                            if ref["kind"] != "playwright" or ref["test"] != spec["title"]:
                                continue
                            file = spec.get("file", suite.get("file", "")).replace("\\", "/")
                            # Playwright's file is relative to testDir, or absolute; allow either.
                            candidates = {ref["file"], ref.get("report_file", ref["file"])}
                            resolved = (ROOT / "tests/e2e" / file).resolve()
                            matches = any(file.endswith(candidate) or candidate.endswith("/" + file) or
                                          resolved == (ROOT / candidate).resolve() for candidate in candidates)
                            if matches:
                                evidence[("playwright", ref["file"], ref["test"])].add(test.get("projectName", ""))
                visit(suite.get("suites", []))

        visit(data.get("suites", []))
    for path in script_paths:
        data = json.loads(Path(path).read_text())
        for item in data.get("observations", []):
            if item.get("outcome") == "pass" and item.get("command") and item.get("artifact"):
                evidence[("script", item["file"], item["test"])].add("script")
    return evidence


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.parse_args()
    errors, checklist = check_mapping()
    for error in errors:
        print(error, file=sys.stderr)
    if not errors:
        print(f"Mapped all {len(checklist['checks'])} PLAN §7 lines; references exist. Execution evidence is checked by run.py.")
    raise SystemExit(bool(errors))
