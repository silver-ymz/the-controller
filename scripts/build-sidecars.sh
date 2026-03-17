#!/usr/bin/env bash
# Build sidecar binaries and copy them to src-tauri/binaries/ with the
# target-triple suffix that Tauri's externalBin expects.
#
# Must run BEFORE `pnpm tauri build` so the binaries are bundled into the app.
# Tauri's build.rs validates that sidecar files exist at compile time, so we
# create placeholders first, build the real binaries, then overwrite them.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
TAURI_DIR="$PROJECT_DIR/src-tauri"
TARGET_TRIPLE="${TAURI_TARGET_TRIPLE:-$(rustc -vV | awk '/^host:/ {print $2}')}"
PROFILE="${1:-release}"

BINARIES=(controller-cli pty-broker)

mkdir -p "$TAURI_DIR/binaries"

# Create placeholders so tauri_build::build() passes validation
for bin in "${BINARIES[@]}"; do
  touch "$TAURI_DIR/binaries/${bin}-${TARGET_TRIPLE}"
done

echo "Building sidecar binaries (profile=$PROFILE, target=$TARGET_TRIPLE)..."
for bin in "${BINARIES[@]}"; do
  cargo build --manifest-path "$TAURI_DIR/Cargo.toml" --"$PROFILE" --bin "$bin"
done

PROFILE_DIR="$( [ "$PROFILE" = "release" ] && echo "release" || echo "debug" )"

for bin in "${BINARIES[@]}"; do
  src="$TAURI_DIR/target/$PROFILE_DIR/$bin"
  dest="$TAURI_DIR/binaries/${bin}-${TARGET_TRIPLE}"
  cp "$src" "$dest"
  echo "  $dest"
done

echo "Done."
