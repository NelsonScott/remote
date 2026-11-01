#!/usr/bin/env bash
# Builds Remote as a Flatpak from this checkout and installs it for the current
# user (no root). The app then shows up in the GNOME launcher like any other.
#
#   scripts/install.sh              build + install (or reinstall)
#   scripts/install.sh --uninstall  remove it again
#
# Needs only `flatpak`. The GNOME SDK, the Rust SDK extension and
# flatpak-builder (as the org.flatpak.Builder app, unless a host
# flatpak-builder exists) are installed per-user from Flathub on first run.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_ID="xyz.scottnelson.Remote"
MANIFEST="${ROOT}/packaging/flatpak/${APP_ID}.json"
WORK="${XDG_CACHE_HOME:-${HOME}/.cache}/remote-dev"

if [[ "${1:-}" == "--uninstall" ]]; then
    flatpak uninstall --user -y --noninteractive "${APP_ID}"
    exit 0
fi

flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo

if command -v flatpak-builder >/dev/null; then
    builder=(flatpak-builder)
else
    if ! flatpak info org.flatpak.Builder >/dev/null 2>&1; then
        flatpak install --user -y --noninteractive flathub org.flatpak.Builder
    fi
    builder=(flatpak run org.flatpak.Builder)
fi

mkdir -p "${WORK}"
"${builder[@]}" --user --install-deps-from=flathub --force-clean \
    --state-dir="${WORK}/flatpak-state" --repo="${WORK}/flatpak-repo" \
    "${WORK}/flatpak-build" "${MANIFEST}"

flatpak install --user -y --noninteractive --reinstall "${WORK}/flatpak-repo" "${APP_ID}"

echo
echo "installed ${APP_ID} (user). Launch it from the app grid, or: flatpak run ${APP_ID}"
