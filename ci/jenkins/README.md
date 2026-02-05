# Jenkins: REG Linux Release Publishing

This directory contains the bash scripts that implement the **manual tag** release flow.
They are copy/paste friendly and do not require custom Jenkins plugins.

## Prereqs on Jenkins Agents
Tools required:
- `bash`
- `zstd`
- `split`
- `sha256sum`
- `jq`
- `gh` (GitHub CLI)

Auth:
- Set `GITHUB_TOKEN` in Jenkins credentials (env var).
- Token needs permission to create and upload release assets in `REG-Linux/REG-Linux`.

## Target Discovery Rule
A target is any directory under `board/` that contains `genimage.cfg`.
Target name is the folder name containing that file.

```bash
find board -type f -name genimage.cfg -print | awk -F/ '{print $(NF-1)}' | sort -u
```

## Per-Target Publish (parallel)
Script: `ci/jenkins/per_target_publish.sh`

Required env:
- `REPO` (e.g. `REG-Linux/REG-Linux`)
- `TAG` (manual tag, e.g. `v1.0-rc1`)
- `TARGET` (e.g. `cha`)
- `IMG_PATH` (path to the built `.img` file)
- `OUTDIR` (output directory for compressed + split files)

Optional:
- `SPLIT_SIZE_MB` (default `1900`)
- `ZSTD_LEVEL` (default `19`)

Example:
```bash
export REPO=REG-Linux/REG-Linux
export TAG=v1.0-rc1
export TARGET=cha
export IMG_PATH=/build/output/reglinux.img
export OUTDIR=dist

ci/jenkins/per_target_publish.sh
```

Outputs:
- `reglinux-<target>-<tag>.img.zst`
- `reglinux-<target>-<tag>.img.zst.part###`
- `meta-<target>.json`
- `reglinux-<target>-<tag>.parts.sha256`

The script uploads parts + meta to the GitHub Release.

## Aggregate Release (after all targets)
Script: `ci/jenkins/aggregate_release.sh`

Required env:
- `REPO`
- `TAG`

Optional:
- `WORKDIR` (default `agg`)

Example:
```bash
export REPO=REG-Linux/REG-Linux
export TAG=v1.0-rc1

ci/jenkins/aggregate_release.sh
```

Outputs:
- `manifest.json` (global manifest)
- `SHA256SUMS` (all parts, deterministic order)

The script downloads all `meta-*.json` assets from the release, rebuilds
`manifest.json` and `SHA256SUMS`, then uploads them (overwriting if present).
