#!/bin/sh
set -eu

# Deterministic release bundler for Metron (.tar.zst, .deb, SHA256SUMS, RELEASE-EVIDENCE.json).
# Zero Python: uses only POSIX sh, tar, zstd, sha256sum, jq, and dpkg-deb.

VERSION=""
TARGET="x86_64-unknown-linux-gnu"
BIN_DIR=""
OUT_DIR=""
COMMIT="unknown"
REQUIRE_DEB=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --target) TARGET="$2"; shift 2 ;;
    --bin-dir) BIN_DIR="$2"; shift 2 ;;
    --out-dir) OUT_DIR="$2"; shift 2 ;;
    --commit) COMMIT="$2"; shift 2 ;;
    --require-deb) REQUIRE_DEB=1; shift ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

[ -n "$VERSION" ] || { echo "--version required" >&2; exit 2; }
[ -n "$BIN_DIR" ] || { echo "--bin-dir required" >&2; exit 2; }
[ -n "$OUT_DIR" ] || { echo "--out-dir required" >&2; exit 2; }

for bin in metron-runtime metron-jail metron-wasm-compiler; do
  [ -x "$BIN_DIR/$bin" ] || { echo "missing executable $BIN_DIR/$bin" >&2; exit 1; }
done

mkdir -p "$OUT_DIR"
CLEAN_VER="${VERSION#v}"
STAGE_NAME="metron-${VERSION}-${TARGET}"
STAGE_DIR="$OUT_DIR/$STAGE_NAME"
rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR/bin" "$STAGE_DIR/docs"

for bin in metron-runtime metron-jail metron-wasm-compiler; do
  cp "$BIN_DIR/$bin" "$STAGE_DIR/bin/$bin"
  chmod 0755 "$STAGE_DIR/bin/$bin"
done
cp README.md LICENSE "$STAGE_DIR/"
if [ -f "docs/releases/${VERSION}.md" ]; then
  cp "docs/releases/${VERSION}.md" "$STAGE_DIR/docs/"
fi

TAR_NAME="${STAGE_NAME}.tar.zst"
(
  cd "$OUT_DIR"
  tar --sort=name --mtime='@0' --owner=0 --group=0 --numeric-owner -cf - "$STAGE_NAME" | zstd -19 -T0 -q -o "$TAR_NAME"
)

DEB_NAME=""
if command -v dpkg-deb >/dev/null 2>&1; then
  DEB_VER="$(printf '%s' "$CLEAN_VER" | tr '-' '~')"
  DEB_NAME="metron_${DEB_VER}_amd64.deb"
  DEB_ROOT="$OUT_DIR/deb-root"
  rm -rf "$DEB_ROOT"
  mkdir -p "$DEB_ROOT/DEBIAN" "$DEB_ROOT/usr/bin" "$DEB_ROOT/usr/share/doc/metron"
  for bin in metron-runtime metron-jail metron-wasm-compiler; do
    cp "$BIN_DIR/$bin" "$DEB_ROOT/usr/bin/$bin"
    chmod 0755 "$DEB_ROOT/usr/bin/$bin"
  done
  cp README.md LICENSE "$DEB_ROOT/usr/share/doc/metron/"
  cat > "$DEB_ROOT/DEBIAN/control" <<EOF
Package: metron
Version: $DEB_VER
Section: devel
Priority: optional
Architecture: amd64
Maintainer: Metron Contributors <47609243+kmosoti@users.noreply.github.com>
Homepage: https://github.com/kmosoti/Metron
Description: Rust-native agent harness with Wasmtime 51 AOT and Landlock/seccomp isolation
 Metron provides an authority-bounded agent harness, two-lane zero-bubblewrap
 sandbox (Wasmtime 51 AOT + Landlock/seccomp-bpf), rmcp 3.11 MCP bridge,
 and OpenAI/Anthropic HTTP model provider adapters.
EOF
  dpkg-deb --root-owner-group --build "$DEB_ROOT" "$OUT_DIR/$DEB_NAME" >/dev/null
  rm -rf "$DEB_ROOT"
elif [ "$REQUIRE_DEB" -eq 1 ]; then
  echo "dpkg-deb is required for .deb packaging" >&2
  exit 1
fi

rm -rf "$STAGE_DIR"

(
  cd "$OUT_DIR"
  rm -f SHA256SUMS RELEASE-EVIDENCE.json
  LC_ALL=C sha256sum "$TAR_NAME" ${DEB_NAME:+"$DEB_NAME"} > SHA256SUMS
  ARTIFACTS_JSON="$(jq -Rn '[inputs | split("  ") | {file: .[1], sha256: .[0]}]' < SHA256SUMS)"
  jq -n \
    --arg version "$VERSION" \
    --arg target "$TARGET" \
    --arg commit "$COMMIT" \
    --arg rust_toolchain "1.100.0" \
    --arg generated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --argjson artifacts "$ARTIFACTS_JSON" \
    '{
      schema: "metron.release-evidence.v1",
      version: $version,
      target: $target,
      commit: $commit,
      rust_toolchain: $rust_toolchain,
      generated_at_utc: $generated_at,
      sandbox_lanes: {
        lane_a: "wasmtime-51-aot + wasmtime-wasi-51 + metron-wasm-aot-loader (<80 lines) + Landlock + seccomp(PROT_EXEC denied post-load)",
        lane_b: "systemd-run --user --collect + PR_SET_NO_NEW_PRIVS + Landlock ABI v3..v6 + seccomp-bpf + sealed memfd (zero bwrap)"
      },
      artifacts: $artifacts
    }' > RELEASE-EVIDENCE.json
)
