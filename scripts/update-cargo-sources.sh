#!/usr/bin/env bash
# Regenerates packaging/flatpak/cargo-sources.json from Cargo.lock.
# Run after any dependency change (Cargo.lock diff), then commit both files.
# Uses flatpak-cargo-generator pinned to a known commit, in a throwaway venv
# under ~/.cache/remote-dev so nothing is installed system-wide.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TOOLS_COMMIT="9d86d336a39b9af38b86f114b6ce5820fe260f91"   # flatpak/flatpak-builder-tools
WORK="${XDG_CACHE_HOME:-${HOME}/.cache}/remote-dev/flatpak-builder-tools-${TOOLS_COMMIT:0:12}"

if [[ ! -x "${WORK}/venv/bin/python" ]]; then
    rm -rf "${WORK}"
    mkdir -p "${WORK}"
    git clone -q https://github.com/flatpak/flatpak-builder-tools "${WORK}/src"
    git -C "${WORK}/src" checkout -q "${TOOLS_COMMIT}"
    python3 -m venv "${WORK}/venv"
    "${WORK}/venv/bin/pip" install -q 'aiohttp==3.12.15' 'tomlkit==0.13.3' 'PyYAML==6.0.2'
fi

"${WORK}/venv/bin/python" -I "${WORK}/src/cargo/flatpak-cargo-generator.py" \
    "${ROOT}/Cargo.lock" -o "${ROOT}/packaging/flatpak/cargo-sources.json"
# cargo deprecated the extensionless `config` file name
sed -i 's/"dest-filename": "config"/"dest-filename": "config.toml"/' "${ROOT}/packaging/flatpak/cargo-sources.json"
echo "updated packaging/flatpak/cargo-sources.json"
