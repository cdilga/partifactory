#!/usr/bin/env python3
"""Generate the harness's two Dex configs from tests/fixtures/identity.toml, so Dex and the
StaticDirectory fixture never drift.

  gen-dex.py <identity.toml> <out-dir>

writes <out-dir>/dex.yaml (issuer from the fixture, normally https://dex.pf.test/dex) and
<out-dir>/dex2.yaml (the second provider: https://dex2.pf.test/dex, its own client, no groups
scope advertised) for the identity bead's second-provider test.

Every static user's password is `password` (test only). Dex has no disabled state: a
`disabled = true` user exists in Dex and partifactory's directory refuses them.
"""
import base64
import json
import os
import sys
import tomllib

# bcrypt("password"), cost 10: Dex's own example hash.
PASSWORD_HASH = "$2a$10$2b2cU8CPhOTaGrs1HRQuAueS7JTT5ZHsHSzYiFPm1leZck7Mc8T4W"


def q(s):
    # JSON strings are valid YAML scalars.
    return json.dumps(s)


def dex_user_id(subject):
    """Decode the fixture's canonical local-connector IDTokenSubject protobuf."""
    raw = base64.urlsafe_b64decode(subject + "=" * (-len(subject) % 4))
    if len(raw) < 3 or raw[0] != 10 or raw[2 + raw[1]:] != bytes([18, 5]) + b"local":
        raise ValueError("fixture subject is not a canonical Dex local subject")
    return raw[2:2 + raw[1]].decode()


def render(issuer, client_id, redirect_hosts, users, with_groups):
    lines = [
        f"issuer: {q(issuer)}",
        "storage:",
        "  type: memory",
        "web:",
        "  http: 0.0.0.0:5556",
        "logger:",
        "  level: info",
        "  format: json",
        "oauth2:",
        "  skipApprovalScreen: true",
        "  alwaysShowLoginScreen: false",
        "expiry:",
        "  idTokens: 10m",
        "  refreshTokens:",
        "    validIfNotUsedFor: 24h",
        "    absoluteLifetime: 72h",
        "enablePasswordDB: true",
        "staticClients:",
        f"  - id: {q(client_id)}",
        f"    name: {q(client_id)}",
        "    secret: harness-client-secret",
        "    redirectURIs:",
    ]
    for h in redirect_hosts:
        lines.append(f"      - {q(h)}")
    lines.append("staticPasswords:")
    for u in users:
        lines += [
            f"  - email: {q(u['email'])}",
            f"    hash: {q(PASSWORD_HASH)}",
            f"    username: {q(u['username'])}",
            f"    name: {q(u.get('display_name', u['username']))}",
            f"    userID: {q(dex_user_id(u['sub']))}",
        ]
        if with_groups and u.get("groups"):
            lines.append("    groups:")
            for g in u["groups"]:
                lines.append(f"      - {q(g)}")
    return "\n".join(lines) + "\n"


def main():
    src, out = sys.argv[1], sys.argv[2]
    with open(src, "rb") as f:
        fx = tomllib.load(f)
    users = fx.get("users", [])
    known = {g["name"] for g in fx.get("groups", [])}
    for u in users:
        for g in u.get("groups", []):
            if g not in known:
                sys.exit(f"gen-dex: user {u['username']} is in unknown group {g}")
    apex = os.environ.get("PF_APEX", "pf.test")
    redirects = [
        "https://parts.pf.test/oidc/callback",
        "https://parts.pf.test/oauth/callback",
        "https://parts.pf.test/auth/callback",
        "https://parts.pf.test/_dev/callback",
    ]
    redirects = [u.replace("pf.test", apex) for u in redirects]
    issuer = fx.get("issuer", "https://dex.pf.test/dex").replace("pf.test", apex)
    with open(f"{out}/dex.yaml", "w") as f:
        f.write(render(issuer, "partifactory", redirects, users, True))
    with open(f"{out}/dex2.yaml", "w") as f:
        f.write(render(f"https://dex2.{apex}/dex", "partifactory-second", redirects, users, False))
    print(f"gen-dex: {len(users)} users, {len(known)} groups -> {out}/dex.yaml, dex2.yaml")


if __name__ == "__main__":
    main()
