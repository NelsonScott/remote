#!/usr/bin/env bash
# Runs cargo inside the GNOME SDK Flatpak (the same toolchain and headers the
# Flatpak build uses), so hacking on Remote needs no gtk4-devel, libadwaita-devel
# or Rust on the host. Only `flatpak` is required.
#
#   scripts/dev.sh test
#   scripts/dev.sh clippy --all-targets -- -D warnings
#   scripts/dev.sh fmt --check
#   ROKU_MOCK=1 scripts/dev.sh run
#
# ROKU_*, RUST_LOG, GDK_BACKEND, GSK_RENDERER, ADW_DEBUG_COLOR_SCHEME and GTK_THEME are
# forwarded. Without WAYLAND_DISPLAY (e.g. under Xvfb) the X11 socket is used.
# The sandbox has its own /tmp: point ROKU_MOCK_LOG somewhere under the repo
# (target/ is git-ignored).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SDK="org.gnome.Sdk//50"
RUST_EXT="org.freedesktop.Sdk.Extension.rust-stable//25.08"
WORK="${XDG_CACHE_HOME:-${HOME}/.cache}/remote-dev"
mkdir -p "${WORK}/cargo-home"

for ref in "${SDK}" "${RUST_EXT}"; do
    if ! flatpak info "${ref}" >/dev/null 2>&1; then
        flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
        flatpak install --user -y --noninteractive flathub "${ref}"
    fi
done

args=(
    --filesystem="${ROOT}"
    --filesystem="${WORK}"
    --filesystem=xdg-config/remote:create
    --filesystem=xdg-cache/remote:create
    --share=network --share=ipc --device=dri
    # xdg-document-portal can crash-loop under heavy flatpak-builder use (exit 21); dev builds never need it.
    --no-documents-portal
    --env=PATH=/usr/lib/sdk/rust-stable/bin:/usr/bin:/bin
    --env=CARGO_HOME="${WORK}/cargo-home"
    --env=CARGO_TARGET_DIR="${ROOT}/target/sdk"
)
if [[ -n "${WAYLAND_DISPLAY:-}" ]]; then
    args+=(--socket=wayland --socket=fallback-x11)
else
    args+=(--nosocket=wayland --socket=x11)
fi
while IFS='=' read -r name _; do
    case "${name}" in
        ROKU_*|RUST_LOG|GDK_BACKEND|GSK_RENDERER|ADW_DEBUG_COLOR_SCHEME|GTK_THEME)
            args+=(--env="${name}=${!name}") ;;
    esac
done < <(env)

cd "${ROOT}"
exec flatpak run "${args[@]}" --cwd="${ROOT}" --command=cargo "${SDK}" "$@"
