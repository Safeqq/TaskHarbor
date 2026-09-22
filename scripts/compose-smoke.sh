#!/usr/bin/env bash
set -Eeuo pipefail

compose=(docker compose -f deploy/compose.yaml)
temporary_directory="$(mktemp -d)"

cleanup() {
  status=$?
  if (( status != 0 )); then
    "${compose[@]}" ps || true
    "${compose[@]}" logs --no-color || true
  fi
  "${compose[@]}" down --volumes --remove-orphans || true
  rm -rf -- "$temporary_directory"
  trap - EXIT
  exit "$status"
}
trap cleanup EXIT

: "${TASKHARBOR_OWNER_PASSWORD:?set TASKHARBOR_OWNER_PASSWORD before running the smoke test}"

"${compose[@]}" up --detach --build --wait

base_url="https://127.0.0.1:8443"
cookie_jar="$temporary_directory/cookies.txt"
source_image="$temporary_directory/source.png"
downloaded_image="$temporary_directory/output.jpg"

curl --fail --insecure --silent --show-error "$base_url/health/live" >/dev/null
curl --fail --insecure --silent --show-error "$base_url/health/ready" >/dev/null
index_html="$(curl --fail --insecure --silent --show-error "$base_url/")"
grep --quiet "TaskHarbor" <<<"$index_html"

python3 - "$source_image" <<'PY'
import binascii
import struct
import sys
import zlib

path = sys.argv[1]
width, height = 4, 2
rows = []
for y in range(height):
    row = bytearray([0])
    for x in range(width):
        row.extend((40 + x * 30, 80 + y * 40, 180, 255))
    rows.append(bytes(row))

def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", binascii.crc32(kind + data) & 0xFFFFFFFF)

png = b"\x89PNG\r\n\x1a\n"
png += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(b"".join(rows), 9))
png += chunk(b"IEND", b"")
with open(path, "wb") as output:
    output.write(png)
PY

login_body="$(TASKHARBOR_OWNER_PASSWORD="$TASKHARBOR_OWNER_PASSWORD" python3 - <<'PY'
import json
import os

print(json.dumps({"username": "owner", "password": os.environ["TASKHARBOR_OWNER_PASSWORD"]}))
PY
)"
login_response="$(curl --fail --insecure --silent --show-error \
  --cookie-jar "$cookie_jar" \
  --header "Content-Type: application/json" \
  --data "$login_body" \
  "$base_url/api/v1/session/login")"
csrf_token="$(python3 -c 'import json, sys; print(json.load(sys.stdin)["csrf_token"])' <<<"$login_response")"

job_response="$(curl --fail --insecure --silent --show-error \
  --cookie "$cookie_jar" \
  --header "X-CSRF-Token: $csrf_token" \
  --header "Idempotency-Key: compose-smoke-job" \
  --form "name=Compose smoke image" \
  --form "max_width=2" \
  --form "jpeg_quality=85" \
  --form "images=@$source_image;type=image/png" \
  "$base_url/api/v1/jobs")"
job_id="$(python3 -c 'import json, sys; print(json.load(sys.stdin)["id"])' <<<"$job_response")"

job_status=""
job_detail=""
for _ in {1..60}; do
  job_detail="$(curl --fail --insecure --silent --show-error \
    --cookie "$cookie_jar" \
    "$base_url/api/v1/jobs/$job_id")"
  job_status="$(python3 -c 'import json, sys; print(json.load(sys.stdin)["status"])' <<<"$job_detail")"
  case "$job_status" in
    succeeded) break ;;
    failed|cancelled)
      printf 'job %s reached unexpected status %s\n' "$job_id" "$job_status" >&2
      exit 1
      ;;
  esac
  sleep 1
done

if [[ "$job_status" != "succeeded" ]]; then
  printf 'job %s did not complete within 60 seconds; last status: %s\n' "$job_id" "$job_status" >&2
  exit 1
fi

download_path="$(python3 -c 'import json, sys; print(json.load(sys.stdin)["outputs"][0]["download_url"])' <<<"$job_detail")"
curl --fail --insecure --silent --show-error \
  --cookie "$cookie_jar" \
  --output "$downloaded_image" \
  "$base_url$download_path"
python3 - "$downloaded_image" <<'PY'
import sys

with open(sys.argv[1], "rb") as image:
    if image.read(2) != b"\xff\xd8":
        raise SystemExit("downloaded output is not a JPEG")
PY

printf 'Compose smoke test completed job %s through the HTTPS web gateway.\n' "$job_id"
