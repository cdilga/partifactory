#!/usr/bin/env bash
# Internal CA + wildcard leaf for the spike rig. Idempotent: reuses existing certs.
# Output: target/sandbox-spike/certs/{ca.pem,ca.key,cert.pem,key.pem,spki.txt}
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${SPIKE_OUT:-$HERE/../../target/sandbox-spike}/certs"
mkdir -p "$OUT"
cd "$OUT"

if [[ -f cert.pem && -f key.pem && -f ca.pem && -f spki.txt ]]; then
  echo "certs: reusing $OUT"
  exit 0
fi

SANS=""
for apex in pf.test localtest.me; do
  SANS+="DNS:$apex,DNS:*.$apex,"
  for via in caddy nginx nginxnaive traefik; do
    SANS+="DNS:*.$via.$apex,"
  done
done
SANS="${SANS%,}"

openssl req -x509 -newkey rsa:2048 -nodes -keyout ca.key -out ca.pem -days 825 \
  -subj "/CN=partifactory sandbox spike CA" \
  -addext "basicConstraints=critical,CA:TRUE" \
  -addext "keyUsage=critical,keyCertSign,cRLSign" 2>/dev/null

openssl req -newkey rsa:2048 -nodes -keyout key.pem -out leaf.csr -subj "/CN=pf.test" 2>/dev/null
printf 'basicConstraints=CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=%s\n' "$SANS" > leaf.ext
# 365 days: Apple platforms reject TLS leaf certs valid for more than 398 days.
openssl x509 -req -in leaf.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out cert.pem -days 365 -sha256 -extfile leaf.ext 2>/dev/null
cat cert.pem ca.pem > fullchain.pem

# SPKI hash of the leaf, for Chromium's --ignore-certificate-errors-spki-list (treats the
# cert as valid, so WebAuthn and secure-context features behave as on a trusted cert).
openssl x509 -in cert.pem -pubkey -noout | openssl pkey -pubin -outform der \
  | openssl dgst -sha256 -binary | openssl enc -base64 > spki.txt
echo "certs: generated in $OUT"
