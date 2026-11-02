#!/bin/sh
# Generates a personal dev JWT keypair + JWKS + sample token for the local API.
#
# The old workflow kept a shared dev keypair in git, which meant anyone with a
# clone could forge tokens against any deployment that mounted it. Keys are now
# generated per developer, live only on disk (gitignored), and never leave your
# machine. Regenerate with:
#
#   ./dev/gen-dev-keys.sh [--force]
#
# Requires openssl 1.1+ and python3 in PATH (or: nix-shell -p openssl python3).
#
# Outputs (in ./dev/, gitignored):
#   dev-key.pem   RSA-2048 private key (PKCS#8)
#   jwks.json     matching public JWKS, kid dev-key-1
#   token.txt     sample token signed by the fresh key
#
# --force regenerates even if a keypair already exists.

set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$here"

if [ -f dev-key.pem ] && [ "${1:-}" != "--force" ]; then
  echo "dev/dev-key.pem already exists; refusing to overwrite." >&2
  echo "Re-run with --force to regenerate (old key + JWKS are replaced)." >&2
  exit 1
fi

if ! command -v openssl >/dev/null 2>&1; then
  echo "openssl not found in PATH." >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 not found in PATH." >&2
  exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out dev-key.pem 2>/dev/null
openssl pkey -in dev-key.pem -pubout -outform DER -out "$tmp/pub.der"

# jwks.json + token.txt, both derived from the fresh key.
python3 - "$tmp/pub.der" <<'EOF'
import base64
import json
import subprocess
import sys
import time

der = open(sys.argv[1], "rb").read()


def read_tlv(data, off):
    """Returns (value_bytes, next_off) for the DER TLV starting at off."""
    ln = data[off + 1]
    if ln & 0x80:
        cnt = ln & 0x7F
        ln = int.from_bytes(data[off + 2 : off + 2 + cnt], "big")
        return data[off + 2 + cnt : off + 2 + cnt + ln], off + 2 + cnt + ln
    return data[off + 2 : off + 2 + ln], off + 2 + ln


def read_int(data, off):
    """Reads a DER INTEGER, returns (value, next_off)."""
    if data[off] != 0x02:
        sys.exit("unexpected DER: expected INTEGER, got %#x" % data[off])
    val, off = read_tlv(data, off)
    return int.from_bytes(val, "big"), off


def b64url(raw):
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()


if der[0] != 0x30:
    sys.exit("unexpected DER: not a SEQUENCE")

# SubjectPublicKeyInfo ::= SEQUENCE {
#   algorithm          AlgorithmIdentifier,  -- skipped wholesale
#   subjectPublicKey   BIT STRING            -- wraps RSAPublicKey DER
# }
spki_body, _ = read_tlv(der, 0)      # content of the outer SEQUENCE
_, off = read_tlv(spki_body, 0)      # skip AlgorithmIdentifier (a SEQUENCE)
if spki_body[off] != 0x03:
    sys.exit("unexpected DER: no subjectPublicKey BIT STRING")
bitstring, _ = read_tlv(spki_body, off)
rsa_pub = bitstring[1:]              # strip the unused-bits byte

# RSAPublicKey ::= SEQUENCE { modulus INTEGER, publicExponent INTEGER }
if rsa_pub[0] != 0x30:
    sys.exit("unexpected DER: RSAPublicKey not a SEQUENCE")
rsa_body, _ = read_tlv(rsa_pub, 0)
modulus, m_end = read_int(rsa_body, 0)
exponent, _ = read_int(rsa_body, m_end)

jwk = {
    "kty": "RSA",
    "alg": "RS256",
    "use": "sig",
    "kid": "dev-key-1",
    "n": b64url(modulus.to_bytes((modulus.bit_length() + 7) // 8, "big")),
    "e": b64url(exponent.to_bytes((exponent.bit_length() + 7) // 8, "big")),
}
with open("jwks.json", "w") as f:
    json.dump({"keys": [jwk]}, f)
    f.write("\n")

# Sample token signed by the fresh key (exp = now + 90 days).
now = int(time.time())
header = {"alg": "RS256", "typ": "JWT", "kid": "dev-key-1"}
claims = {
    "sub": "user-alice",
    "iss": "https://dev-idp.example.com",
    "aud": "twt-api",
    "iat": now,
    "exp": now + 90 * 24 * 3600,
    "scope": "twt.data",
}
signing_input = (
    b64url(json.dumps(header, separators=(",", ":")).encode())
    + "."
    + b64url(json.dumps(claims, separators=(",", ":")).encode())
)
sig = subprocess.run(
    ["openssl", "dgst", "-sha256", "-sign", "dev-key.pem"],
    input=signing_input.encode(),
    capture_output=True,
    check=True,
).stdout
with open("token.txt", "w") as f:
    f.write(signing_input + "." + b64url(sig) + "\n")
EOF

echo "Generated dev/dev-key.pem, dev/jwks.json, dev/token.txt"
