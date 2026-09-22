#!/bin/sh
set -eu

certificate=/etc/nginx/tls/taskharbor.crt
private_key=/etc/nginx/tls/taskharbor.key

if [ ! -s "$certificate" ] || [ ! -s "$private_key" ]; then
  rm -f "$certificate" "$private_key"
  openssl req \
    -x509 \
    -newkey rsa:2048 \
    -sha256 \
    -nodes \
    -days 30 \
    -subj "/CN=localhost" \
    -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" \
    -keyout "$private_key" \
    -out "$certificate"
  chmod 600 "$private_key"
fi

