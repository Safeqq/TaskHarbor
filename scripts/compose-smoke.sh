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
health_response="$temporary_directory/health.json"
index_response="$temporary_directory/index.html"
login_response="$temporary_directory/login.json"
job_response="$temporary_directory/job.json"
job_detail="$temporary_directory/job-detail.json"

request() {
  local label="$1"
  local failure_code="$2"
  local output_path="$3"
  shift 3

  local http_status
  if ! http_status="$(curl --insecure --silent --show-error \
    --output "$output_path" \
    --write-out "%{http_code}" \
    "$@")"; then
    printf '%s request could not be completed\n' "$label" >&2
    exit "$failure_code"
  fi
  if [[ ! "$http_status" =~ ^2[0-9][0-9]$ ]]; then
    printf '%s request returned HTTP %s\n' "$label" "$http_status" >&2
    if [[ -s "$output_path" ]]; then
      head --bytes 4096 "$output_path" >&2
      printf '\n' >&2
    fi
    exit "$failure_code"
  fi
}

request "liveness" 31 "$health_response" "$base_url/health/live"
request "readiness" 32 "$health_response" "$base_url/health/ready"
request "web index" 33 "$index_response" "$base_url/"
if ! grep --quiet "TaskHarbor" "$index_response"; then
  printf 'web index did not contain the TaskHarbor marker\n' >&2
  exit 34
fi

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
request "login" 41 "$login_response" \
  --cookie-jar "$cookie_jar" \
  --header "Content-Type: application/json" \
  --data "$login_body" \
  "$base_url/api/v1/session/login"
if ! csrf_token="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["csrf_token"])' "$login_response")"; then
  printf 'login response did not contain a CSRF token\n' >&2
  exit 42
fi

request "job creation" 51 "$job_response" \
  --cookie "$cookie_jar" \
  --header "X-CSRF-Token: $csrf_token" \
  --header "Idempotency-Key: compose-smoke-job" \
  --form "name=Compose smoke image" \
  --form "max_width=2" \
  --form "jpeg_quality=85" \
  --form "images=@$source_image;type=image/png" \
  "$base_url/api/v1/jobs"
if ! job_id="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["id"])' "$job_response")"; then
  printf 'job creation response did not contain an ID\n' >&2
  exit 52
fi

job_status=""
for _ in {1..60}; do
  request "job detail" 53 "$job_detail" \
    --cookie "$cookie_jar" \
    "$base_url/api/v1/jobs/$job_id"
  if ! job_status="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["status"])' "$job_detail")"; then
    printf 'job detail response did not contain a status\n' >&2
    exit 54
  fi
  case "$job_status" in
    succeeded) break ;;
    failed|cancelled)
      printf 'job %s reached unexpected status %s\n' "$job_id" "$job_status" >&2
      exit 55
      ;;
  esac
  sleep 1
done

if [[ "$job_status" != "succeeded" ]]; then
  printf 'job %s did not complete within 60 seconds; last status: %s\n' "$job_id" "$job_status" >&2
  exit 56
fi

if ! download_path="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["outputs"][0]["download_url"])' "$job_detail")"; then
  printf 'succeeded job response did not contain an output download URL\n' >&2
  exit 57
fi
request "output download" 61 "$downloaded_image" \
  --cookie "$cookie_jar" \
  "$base_url$download_path"
if ! python3 - "$downloaded_image" <<'PY'
import sys

with open(sys.argv[1], "rb") as image:
    if image.read(2) != b"\xff\xd8":
        raise SystemExit("downloaded output is not a JPEG")
PY
then
  exit 62
fi

printf 'Compose smoke test completed job %s through the HTTPS web gateway.\n' "$job_id"
