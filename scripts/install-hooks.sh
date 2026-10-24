#!/usr/bin/env bash
# Install the pre-push hook that runs gitleaks before every push (AGENTS.md "Secrets").
# The Agent Mail pre-commit guard is installed separately and is left alone.
#   PF_HOOKS_REPO  the checkout to install into (default: this one; the self-test uses it)
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
repo="${PF_HOOKS_REPO:-$here}"
hooks="$(git -C "$repo" rev-parse --absolute-git-dir)/hooks"
mkdir -p "$hooks"
cat > "$hooks/pre-push" <<EOF
#!/usr/bin/env bash
# Installed by partifactory scripts/install-hooks.sh: gitleaks before every push.
exec "$here/ci/gitleaks.sh" "\$(git rev-parse --show-toplevel)"
EOF
chmod +x "$hooks/pre-push"
echo "install-hooks: pre-push gitleaks hook installed in $hooks"
