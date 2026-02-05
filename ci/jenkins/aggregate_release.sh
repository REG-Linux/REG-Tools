#!/usr/bin/env bash
set -euo pipefail

# Required env:
#   REPO="REG-Linux/REG-Linux"
#   TAG="v1.0-rc1"   (manual tag)
#
# Optional:
#   WORKDIR="agg"

: "${REPO:?Missing REPO}"
: "${TAG:?Missing TAG}"

WORKDIR="${WORKDIR:-agg}"
mkdir -p "$WORKDIR/metas"
cd "$WORKDIR"

echo "[1/5] Download meta-*.json from release..."
rm -f metas/meta-*.json || true
gh release download "$TAG" -R "$REPO" --pattern 'meta-*.json' --dir metas

echo "[2/5] Ensure we have at least one meta file..."
count=$(ls -1 metas/meta-*.json 2>/dev/null | wc -l | tr -d ' ')
if [ "${count}" = "0" ]; then
  echo "ERROR: No meta-*.json found in release $TAG. Aborting."
  exit 2
fi

echo "[3/5] Build manifest.json..."
jq -s \
  --arg repo "$REPO" \
  --arg tag "$TAG" \
  --arg generated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  '
  {
    schema: 1,
    repo: $repo,
    tag: $tag,
    generated_at: $generated_at,
    images: (map({key: .target, value: .}) | from_entries)
  }' metas/meta-*.json > manifest.json

echo "[4/5] Build SHA256SUMS..."
jq -r '.parts[] | "\(.sha256)  \(.name)"' metas/meta-*.json \
  | LC_ALL=C sort > SHA256SUMS

echo "[5/5] Upload manifest.json + SHA256SUMS..."
gh release upload "$TAG" -R "$REPO" manifest.json SHA256SUMS --clobber

echo "Done."
