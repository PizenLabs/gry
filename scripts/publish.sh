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
#   PUBLISH_DELAY=20  seconds between registry index polls / retries
#   WAIT_TIMEOUT=900  seconds to wait for a crate to reach the registry index
#   MAX_ATTEMPTS=3    attempts per crate before giving up
#   CARGO_REGISTRY_TOKEN  crates.io API token (required unless DRY_RUN=1)
#
# Always publish through this script (or `make publish`) rather than a bare
# `cargo publish` at the workspace root. A bare workspace publish makes Cargo
# wait for every just-uploaded crate to appear in the index before it uploads
# the next one, and Cargo's built-in wait is short enough that crates.io
# propagation frequently times out, aborting the remaining crates.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

CRATES=(gry-core gry-store gry-parse gry-index gry-query gry-mcp gry)
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
DRY_RUN="${DRY_RUN:-0}"
SKIP_PUBLISHED="${SKIP_PUBLISHED:-1}"
PUBLISH_DELAY="${PUBLISH_DELAY:-20}"
WAIT_TIMEOUT="${WAIT_TIMEOUT:-900}"
MAX_ATTEMPTS="${MAX_ATTEMPTS:-3}"
EXTRA_ARGS=("$@")
USER_AGENT="gry-publish (https://github.com/PizenLabs/gry)"
INDEX_URL="${INDEX_URL:-https://index.crates.io}"

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

# Path of a crate inside the sparse registry index, following the layout rules
# from the registry index format documentation.
index_path() {
  local crate="$1" length="${#1}"
  if [ "$length" -eq 1 ]; then
    printf '1/%s' "$crate"
  elif [ "$length" -eq 2 ]; then
    printf '2/%s' "$crate"
  elif [ "$length" -eq 3 ]; then
    printf '3/%s/%s' "${crate:0:1}" "$crate"
  else
    printf '%s/%s/%s' "${crate:0:2}" "${crate:2:2}" "$crate"
  fi
}

# True once the registry index serves this exact version, which is what Cargo
# needs before a dependent crate can be published.
indexed() {
  local crate="$1" body
  body="$(curl -sS -f -H "User-Agent: ${USER_AGENT}" \
    "${INDEX_URL}/$(index_path "$crate")" 2>/dev/null || true)"
  grep -qF "\"vers\":\"${VERSION}\"" <<<"$body"
}

# A crate counts as done if either the API or the index already has it.
done_publishing() {
  local crate="$1"
  published "$crate" || indexed "$crate"
}

# Report a crate that crates.io accepted but the sparse index does not serve,
# which is the state that makes dependents fail with "no matching package".

wait_for_index() {
  local crate="$1" waited=0
  [ "$DRY_RUN" = "1" ] && return 0
  log "waiting up to ${WAIT_TIMEOUT}s for crates.io to serve ${crate} ${VERSION}"
  while ! indexed "$crate"; do
    if [ "$waited" -ge "$WAIT_TIMEOUT" ]; then
      log "warning: ${crate} ${VERSION} is still not in the index; continuing"
      return 1
    fi
    sleep "$PUBLISH_DELAY"
    waited=$((waited + PUBLISH_DELAY))
  done
  log "${crate} ${VERSION} is available in the index"
}

# Wait until every internal dependency of a crate is served by the index.
# Publishing a crate whose dependency is only in the API but not yet in the
# index fails with "no matching package".
wait_for_deps() {
  local crate="$1" dep
  for dep in "${CRATES[@]}"; do
    [ "$dep" = "$crate" ] && return 0
    if done_publishing "$dep"; then
      if indexed "$dep"; then
        continue
      fi
      if published "$dep"; then
        log "${dep} ${VERSION} is on crates.io but not in the registry index yet"
      fi
      wait_for_index "$dep" || return 1
    fi
  done
  return 0
}

# Fail the run unless every workspace crate is resolvable through the index.
verify_index() {
  local crate missing=()
  for crate in "${CRATES[@]}"; do
    if indexed "$crate"; then
      log "verified ${crate} ${VERSION} in the registry index"
    elif published "$crate" && wait_for_index "$crate"; then
      log "verified ${crate} ${VERSION} in the registry index"
    else
      missing+=("$crate")
      log "error: ${crate} ${VERSION} is not in the registry index" >&2
    fi
  done
  if [ "${#missing[@]}" -ne 0 ]; then
    die "missing from the registry index: ${missing[*]}; retry later or check https://status.crates.io"
  fi
}

publish_one() {
  local crate="$1" attempt=1 status log_file propagation missing_dep current_version
  current_version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
  [ -n "$current_version" ] || die "could not read the workspace version from Cargo.toml"
  if [ "$current_version" != "$VERSION" ]; then
    die "workspace version changed from ${VERSION} to ${current_version} during the run; rerun the script so the remaining crates are published at ${current_version}"
  fi
  if [ "$SKIP_PUBLISHED" = "1" ] && published "$crate"; then
    if wait_for_index "$crate"; then
      log "${crate} ${VERSION} is already on crates.io, skipping"
      return 0
    fi
    die "${crate} ${VERSION} is uploaded but missing from the registry index; cannot continue"
  fi
  if [ "$SKIP_PUBLISHED" = "1" ] && indexed "$crate"; then
    log "${crate} ${VERSION} is already on crates.io, skipping"
    return 0
  fi

  while :; do
    log_file="$(mktemp "${TMPDIR:-/tmp}/gry-publish.XXXXXX")"
    log "publishing ${crate} ${VERSION} (attempt ${attempt}/${MAX_ATTEMPTS})"
    set +e
    cargo publish -p "$crate" --locked ${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"} 2>&1 |
      tee "$log_file"
    status="${PIPESTATUS[0]}"
    set -e
    if [ "$status" -eq 0 ]; then
      rm -f "$log_file"
      return 0
    fi

    # Distinguish a propagation timeout (the upload went through) from a real
    # failure such as a verification build error.
    propagation=0
    if grep -qiE \
      'timed out waiting|already (been )?uploaded|waiting for .* to be available|no matching package named|failed to select a version|candidate versions found|too many requests|429' \
      "$log_file"; then
      propagation=1
    fi
    missing_dep="$(sed -n 's/.*no matching package named `\([^`]*\)` found.*/\1/p' "$log_file" | head -n 1)"
    if [ -z "$missing_dep" ]; then
      missing_dep="$(sed -n 's/.*failed to select a version for the requirement `\([A-Za-z0-9_-]*\).*/\1/p' "$log_file" | head -n 1)"
    fi
    rm -f "$log_file"

    if [ "$SKIP_PUBLISHED" = "1" ]; then
      case " ${CRATES[*]} " in
      *" ${missing_dep:-__none__} "*)
        if ! published "$missing_dep"; then
          die "${crate} depends on ${missing_dep} ${VERSION}, which is not on crates.io; publish it first"
        fi
        if ! wait_for_index "$missing_dep"; then
          die "${missing_dep} ${VERSION} is on crates.io but missing from the registry index; retry later or check https://status.crates.io"
        fi
        log "${missing_dep} ${VERSION} is now in the index; retrying ${crate}"
        ;;
      *)
        # The upload itself may have succeeded before the wait timed out.
        if published "$crate"; then
          if wait_for_index "$crate"; then
            log "${crate} ${VERSION} was uploaded; treating as published"
            return 0
          fi
        elif [ "$propagation" = "1" ] && wait_for_deps "$crate"; then
          log "internal dependencies are in the index; retrying ${crate}"
        fi
        ;;
      esac
    fi

    if [ "$attempt" -ge "$MAX_ATTEMPTS" ]; then
      die "failed to publish ${crate} after ${MAX_ATTEMPTS} attempts"
    fi
    attempt=$((attempt + 1))
    log "retrying ${crate} in ${PUBLISH_DELAY}s"
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

for i in "${!CRATES[@]}"; do
  crate="${CRATES[$i]}"
  publish_one "$crate"
done

verify_index

log "done"
