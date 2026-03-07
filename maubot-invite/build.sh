#!/usr/bin/env bash
# Build the maubot plugin into a .mbp (zip) file for upload.
set -euo pipefail

OUT="invite-bot.mbp"

zip -r "$OUT" \
    maubot.yaml \
    base-config.yaml \
    invite_bot/

echo "Built: $OUT"
echo "Upload this file at: https://<your-maubot>/ui/#/plugins"
