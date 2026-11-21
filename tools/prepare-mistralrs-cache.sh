#!/usr/bin/env sh
# Repair the pinned SDK's missing build-script Git metadata watch path.
#
# mistralrs-core/build.rs emits `cargo:rerun-if-changed=.git/HEAD` relative to
# its subdirectory instead of Cargo's Git checkout root, causing spurious
# rebuilds across cached runs. Linking `.git -> ../.git` inside the verified
# mistralrs-core checkout fixes cache reuse without modifying any SDK source.

set -eu

REVISION="fb4cdc3730dc64b366724e7637a205a8b7747499"
SOURCE="git+https://github.com/EricLBuehler/mistral.rs?rev=${REVISION}#${REVISION}"
BUILD_SCRIPT_SHA256="d60cd38d65fc97f479a4d04bcca6db37973c940171c8423d9fb773af71f1fcd6"

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
MANIFEST_PATH="${SCRIPT_DIR}/../Cargo.toml"
TARGET=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --manifest-path)
      MANIFEST_PATH="$2"
      shift 2
      ;;
    --target)
      TARGET="$2"
      shift 2
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      exit 2
      ;;
  esac
done

if [ -z "$TARGET" ]; then
  TARGET="$(rustc -vV | sed -n 's/^host: //p')"
fi

MANIFEST_ARG="$(cargo metadata --locked --offline --filter-platform "$TARGET" --format-version 1 --manifest-path "$MANIFEST_PATH" \
  | jq -er --arg src "$SOURCE" '[.packages[] | select(.name == "mistralrs-core")] | if length == 0 then "none" elif length == 1 then (if .[0].source == $src then .[0].manifest_path else error("SDK source mismatch") end) else error("expected at most one mistralrs-core dependency") end')"

if [ "$MANIFEST_ARG" = "none" ]; then
  printf '{"revision":"%s","action":"skipped_optional_feature_disabled"}\n' "$REVISION"
  exit 0
fi

CORE="$(CDPATH='' cd -- "$(dirname -- "$MANIFEST_ARG")" && pwd)"
CHECKOUT="$(dirname -- "$CORE")"

if [ "$(basename -- "$CORE")" != "mistralrs-core" ]; then
  printf 'unexpected SDK checkout layout: %s\n' "$CORE" >&2
  exit 1
fi
if [ "$(git -C "$CHECKOUT" rev-parse HEAD)" != "$REVISION" ]; then
  printf 'SDK checkout revision mismatch\n' >&2
  exit 1
fi
if [ -n "$(git -C "$CHECKOUT" status --porcelain --untracked-files=no)" ]; then
  printf 'SDK checkout contains modified tracked files\n' >&2
  exit 1
fi

ACTUAL_SHA="$(sha256sum "$CORE/build.rs" | cut -d' ' -f1)"
if [ "$ACTUAL_SHA" != "$BUILD_SCRIPT_SHA256" ]; then
  printf 'SDK build script differs from the pinned version\n' >&2
  exit 1
fi

METADATA="$CHECKOUT/.git"
if [ ! -d "$METADATA" ] || [ ! -f "$METADATA/HEAD" ]; then
  printf 'unsupported Git metadata layout; no files changed\n' >&2
  exit 1
fi

LINK="$CORE/.git"
if [ -L "$LINK" ]; then
  if [ "$(readlink -f "$LINK")" != "$(readlink -f "$METADATA")" ]; then
    printf 'existing SDK metadata link has a different target\n' >&2
    exit 1
  fi
  ACTION="already prepared"
elif [ -e "$LINK" ]; then
  printf 'SDK metadata path already exists; refusing to overwrite\n' >&2
  exit 1
else
  ln -s ../.git "$LINK"
  ACTION="prepared"
fi

if [ "$(git -C "$CORE" rev-parse HEAD)" != "$REVISION" ]; then
  printf 'SDK revision check failed after preparing metadata\n' >&2
  exit 1
fi

printf '{"revision":"%s","metadata_link":"%s","action":"%s"}\n' "$REVISION" "$LINK" "$ACTION"
