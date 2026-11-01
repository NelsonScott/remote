#!/usr/bin/env bash
# Builds a source RPM from the committed HEAD:
#   packaging/rpm/out/remote-<ver>.tar.gz          git archive of HEAD
#   packaging/rpm/out/remote-<ver>-vendor.tar.xz   `cargo vendor --locked`
#   packaging/rpm/out/remote-<ver>-1.<dist>.src.rpm
# Then publish with:  copr-cli build remote packaging/rpm/out/remote-*.src.rpm
# Needs git, rpmbuild (rpm-build) and either cargo or flatpak (scripts/dev.sh).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="${ROOT}/packaging/rpm/out"
VERSION="$(sed -n 's/^Version: *//p' "${ROOT}/packaging/rpm/remote.spec")"
cargo_version="$(sed -n 's/^version = "\(.*\)"/\1/p' "${ROOT}/Cargo.toml" | head -1)"
[[ "${VERSION}" == "${cargo_version}" ]] || { echo "spec ${VERSION} != Cargo.toml ${cargo_version}" >&2; exit 1; }
[[ -z "$(git -C "${ROOT}" status --porcelain)" ]] || echo "warning: uncommitted changes are NOT in the tarball" >&2

rm -rf "${OUT}"
mkdir -p "${OUT}"
git -C "${ROOT}" archive --format=tar.gz --prefix="remote-${VERSION}/" -o "${OUT}/remote-${VERSION}.tar.gz" HEAD

if command -v cargo >/dev/null; then
    (cd "${ROOT}" && cargo vendor --locked --quiet packaging/rpm/out/vendor >/dev/null)
else
    "${ROOT}/scripts/dev.sh" vendor --locked --quiet packaging/rpm/out/vendor >/dev/null
fi
tar -C "${OUT}" -cJf "${OUT}/remote-${VERSION}-vendor.tar.xz" vendor
rm -rf "${OUT}/vendor"

rpmbuild -bs "${ROOT}/packaging/rpm/remote.spec" \
    --define "_sourcedir ${OUT}" --define "_srcrpmdir ${OUT}" \
    --define "_builddir ${OUT}/build" --define "dist .fc$(rpm -E %fedora)"
ls -1 "${OUT}"/*.src.rpm
