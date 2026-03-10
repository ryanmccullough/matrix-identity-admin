#!/usr/bin/env bash
# Provision a Synapse admin token for local e2e testing.
#
# Prerequisites: Docker e2e stack must be running with MAS healthy.
# Usage: ./e2e/provision-admin-token.sh

set -euo pipefail

COMPOSE_FILE="e2e/docker-compose.yml"
TOKEN_FILE="e2e/shared/synapse-admin-token"

echo "Registering testadmin user in MAS..."
docker compose -f "$COMPOSE_FILE" exec -T mas \
  /usr/local/bin/mas-cli manage register-user \
    --username testadmin --admin 2>&1 || true

echo "Issuing Synapse admin compatibility token..."
TOKEN_OUTPUT=$(docker compose -f "$COMPOSE_FILE" exec -T mas \
  /usr/local/bin/mas-cli manage issue-compatibility-token testadmin \
    --yes-i-want-to-grant-synapse-admin-privileges 2>&1)

ADMIN_TOKEN=$(echo "$TOKEN_OUTPUT" | grep -oP 'mct_[A-Za-z0-9_]+' || true)

if [ -z "$ADMIN_TOKEN" ]; then
  echo "ERROR: Failed to extract token from mas-cli output:"
  echo "$TOKEN_OUTPUT"
  exit 1
fi

mkdir -p "$(dirname "$TOKEN_FILE")"
echo "$ADMIN_TOKEN" > "$TOKEN_FILE"
echo "Token written to $TOKEN_FILE"
echo "Token: ${ADMIN_TOKEN:0:15}..."
