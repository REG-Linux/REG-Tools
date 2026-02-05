#!/usr/bin/env bash
set -euo pipefail

# Required env:
#   REPO="REG-Linux/REG-Linux"
#   TAG="v1.0-rc1"              (manual tag)
#   TARGET="cha"
#   IMG_PATH="/abs/path/reglinux.img"
#   OUTDIR="dist"
#
# Optional:
#   SPLIT_SIZE_MB=1900
#   ZSTD_LEVEL=19

: "${REPO:?Missing REPO}"
: "${TAG:?Missing TAG}"
: "${TARGET:?Missing TARGET}"
: "${IMG_PATH:?Missing IMG_PATH}"
: "${OUTDIR:?Missing OUTDIR}"

SPLIT_SIZE_MB="${SPLIT_SIZE_MB:-1900}"
ZSTD_LEVEL="${ZSTD_LEVEL:-19}"

mkdir -p "$OUTDIR"

base="reglinux-${TARGET}-${TAG}"
img_zst="${OUTDIR}/${base}.img.zst"

printf '[bootstrap] Ensure release exists...\n'
if ! gh release view "$TAG" -R "$REPO" >/dev/null 2>&1; then
  gh release create "$TAG" -R "$REPO" --title "$TAG" --notes ""
fi

printf '[1/7] Compress zstd...\n'
zstd -T0 -"${ZSTD_LEVEL}" --long=31 "$IMG_PATH" -o "$img_zst"

printf '[2/7] Split into <2GiB parts...\n'
split -b "${SPLIT_SIZE_MB}m" -d -a 3 "$img_zst" "${img_zst}.part"

printf '[3/7] Compute sha256 for parts...\n'
parts_list="${OUTDIR}/${base}.parts.list"
: > "$parts_list"
for part in "${img_zst}.part"*; do
  name="$(basename "$part")"
  size="$(stat -c%s "$part")"
  sha="$(sha256sum "$part" | awk '{print $1}')"
  printf '%s\t%s\t%s\n' "$name" "$size" "$sha" >> "$parts_list"
done

printf '[4/7] Build meta-%s.json...\n' "$TARGET"
meta_path="${OUTDIR}/meta-${TARGET}.json"
sha256_zst="$(sha256sum "$img_zst" | awk '{print $1}')"
parts_json="$(jq -Rn '[inputs | split("\t") | {name:.[0], size:(.[1]|tonumber), sha256:.[2]}]' "$parts_list")"
bytes_uncompressed="$(stat -c%s "$IMG_PATH")"

jq -n \
  --arg repo "$REPO" \
  --arg tag "$TAG" \
  --arg target "$TARGET" \
  --arg image "$(basename "$img_zst")" \
  --arg created_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg sha256_zst "$sha256_zst" \
  --argjson bytes_uncompressed "$bytes_uncompressed" \
  --argjson parts "$parts_json" \
  '{
    schema: 1,
    repo: $repo,
    tag: $tag,
    target: $target,
    image: $image,
    bytes_uncompressed: $bytes_uncompressed,
    parts: $parts,
    sha256_zst: $sha256_zst,
    created_at: $created_at
  }' > "$meta_path"

printf '[5/7] Upload parts + meta to GitHub Release...\n'
gh release upload "$TAG" -R "$REPO" \
  "${OUTDIR}/${base}.img.zst.part"* \
  "$meta_path" \
  --clobber

printf '[6/7] Upload per-target checksums...\n'
( cd "$OUTDIR" && sha256sum "${base}.img.zst.part"* > "${base}.parts.sha256" )

gh release upload "$TAG" -R "$REPO" "${OUTDIR}/${base}.parts.sha256" --clobber

printf '[7/7] Done.\n'
