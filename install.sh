#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
prefix="${HOME}/.local"

if [[ ! -x "${root}/target/release/kick-lite" ]]; then
    echo "Release binary not found; running cargo build --release..."
    (cd "${root}" && cargo build --release)
fi

install -Dm755 "${root}/target/release/kick-lite" "${prefix}/bin/kick-lite"
install -Dm644 "${root}/io.github.kicklite.KickLite.desktop" \
    "${prefix}/share/applications/io.github.kicklite.KickLite.desktop"
install -Dm644 "${root}/io.github.kicklite.KickLite.svg" \
    "${prefix}/share/icons/hicolor/scalable/apps/io.github.kicklite.KickLite.svg"

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${prefix}/share/applications" >/dev/null 2>&1 || true
fi

echo "Kick Lite installed: ${prefix}/bin/kick-lite"
