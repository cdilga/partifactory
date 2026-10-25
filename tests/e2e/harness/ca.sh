#!/usr/bin/env bash
# A per-run internal CA and a wildcard leaf for *.pf.test (and pf.test), written to $1:
#   ca.pem ca.key        the run's CA (browsers and the iOS Simulator trust this, nothing else)
#   cert.pem key.pem     the leaf Caddy serves (cert.pem holds leaf + CA)
set -euo pipefail
out="${1:?usage: ca.sh <dir>}"
apex="${PF_APEX:-pf.test}"
mkdir -p "$out"
cd "$out"

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout ca.key -out ca.pem -days 2 -subj "/CN=partifactory harness CA $(date +%s)" \
  -addext "basicConstraints=critical,CA:TRUE,pathlen:0" \
  -addext "keyUsage=critical,keyCertSign,cRLSign" 2>/dev/null

openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout key.pem -out leaf.csr -subj "/CN=*.${apex}" 2>/dev/null

cat > leaf.ext <<EOF
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=serverAuth
subjectAltName=DNS:*.${apex},DNS:${apex}
EOF
openssl x509 -req -in leaf.csr -CA ca.pem -CAkey ca.key -CAcreateserial \
  -out leaf.pem -days 2 -extfile leaf.ext 2>/dev/null
cat leaf.pem ca.pem > cert.pem
rm -f leaf.csr leaf.ext ca.srl

chmod 644 ./*.pem
chmod 600 ca.key
echo "ca.sh: CA + *.${apex} leaf in $out"
