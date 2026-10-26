#!/usr/bin/env bash
# Publish the gry workspace to crates.io in dependency order.
#
# Usage:
#   scripts/publish.sh                 # publish every crate that is not already published
#   DRY_RUN=1 scripts/publish.sh       # validate all packages without uploading
#   scripts/publish.sh --no-verify     # extra arguments are forwarded to `cargo publish`
#
# Environment:
#   DRY_RUN=1         validate packaging without uploading
#   SKIP_PUBLISHED=0  fail instead of skipping crates already on crates.io
#   PUBLISH_DELAY=20  seconds to wait for the registry index between crates
#   MAX_ATTEMPTS=3    attempts per crate before giving up
#   CARGO_REGISTRY_TOKEN  crates.io API token (required unless DRY_RUN=1)
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

CRATES=(gry-core gry-store gry-parse gry-index gry-query gry-mcp gry)
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
DRY_RUN="${DRY_RUN:-0}"
SKIP_PUBLISHED="${SKIP_PUBLISHED:-1}"
PUBLISH_DELAY="${PUBLISH_DELAY:-20}"
MAX_ATTEMPTS="${MAX_ATTEMPTS:-3}"
EXTRA_ARGS=("$@")
USER_AGENT="gry-publish (https://github.com/PizenLabs/gry)"

log() { printf 'publish: %s\n' "$*"; }
die() {
  printf 'publish: error: %s\n' "$*" >&2
  exit 1
}

[ -n "$VERSION" ] || die "could not read the workspace version from Cargo.toml"
command -v cargo >/dev/null 2>&1 || die "cargo is not installed"

cargo_home="${CARGO_HOME:-$HOME/.cargo}"
if [ "$DRY_RUN" != "1" ] && [ -z "${CARGO_REGISTRY_TOKEN:-}" ] &&
  [ ! -f "${cargo_home}/credentials.toml" ] && [ ! -f "${cargo_home}/credentials" ]; then
  die "no crates.io credentials; set CARGO_REGISTRY_TOKEN or run 'cargo login'"
fi

if ! command -v curl >/dev/null 2>&1; then
  log "warning: curl not found; cannot check crates.io for published versions"
  SKIP_PUBLISHED=0
fi

published() {
  local crate="$1" status
  status="$(curl -sS -o /dev/null -w '%{http_code}' \
    -H "User-Agent: ${USER_AGENT}" \
    "https://crates.io/api/v1/crates/${crate}/${VERSION}" 2>/dev/null || true)"
  [ "$status" = "200" ]
}

publish_one() {
  local crate="$1" attempt=1
  if [ "$SKIP_PUBLISHED" = "1" ] && published "$crate"; then
    log "${crate} ${VERSION} is already on crates.io, skipping"
    return 0
  fi

  while :; do
    log "publishing ${crate} ${VERSION} (attempt ${attempt}/${MAX_ATTEMPTS})"
    if cargo publish -p "$crate" --locked ${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"}; then
      return 0
    fi

    if [ "$SKIP_PUBLISHED" = "1" ] && published "$crate"; then
      log "${crate} ${VERSION} is already on crates.io, treating as published"
      return 0
    fi

    if [ "$attempt" -ge "$MAX_ATTEMPTS" ]; then
      die "failed to publish ${crate} after ${MAX_ATTEMPTS} attempts"
    fi
    attempt=$((attempt + 1))
    log "retrying ${crate} in ${PUBLISH_DELAY}s (registry propagation)"
    sleep "$PUBLISH_DELAY"
  done
}

log "workspace version ${VERSION}"

if [ "$DRY_RUN" = "1" ]; then
  # A real `cargo publish --dry-run` resolves dependencies from crates.io,
  # which fails until the sibling crates are published. Validate all package
  # contents and metadata together instead. Code correctness is covered by
  # `make verify`.
  log "dry run: validating packages, nothing will be uploaded"
  cargo package --workspace --locked --no-verify ${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"}
  log "done"
  exit 0
fi

last_index=$((${#CRATES[@]} - 1))
for i in "${!CRATES[@]}"; do
  crate="${CRATES[$i]}"
  publish_one "$crate"
  if [ "$i" -lt "$last_index" ]; then
    log "waiting ${PUBLISH_DELAY}s for crates.io to serve ${crate}"
    sleep "$PUBLISH_DELAY"
  fi
done

log "done"
