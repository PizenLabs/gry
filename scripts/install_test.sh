#!/usr/bin/env bash
#
# Deterministic installer tests. No network and no real user configuration.
#
# Usage: bash scripts/install_test.sh
#
# The fixture mirrors the release layout the installer expects:
#   <base>/<asset> and <base>/SHA256SUMS
# with a fake gry binary that records how the installer invoked it.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALLER="$ROOT/scripts/install.sh"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/gry-install-test.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT INT TERM
PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); printf 'ok   %s\n' "$1"; }
fail() { FAIL=$((FAIL + 1)); printf 'FAIL %s\n' "$1" >&2; }

make_release() {
  local rel="$1/latest/download"
  local fake="$WORK/fake-gry"
  cat >"$fake" <<'SH'
#!/bin/sh
case "$1" in
  --version) echo "gry 9.9.9" ;;
  doctor) exit 0 ;;
  install) printf '%s\n' "$*" >>"$GRY_TEST_LOG" ;;
  uninstall) printf '%s\n' "uninstall $*" >>"$GRY_TEST_LOG" ;;
  integrations) echo '{"version":1,"integrations":[]}' ;;
  index) ;;
  *) : ;;
esac
exit 0
SH
  chmod 0755 "$fake"
  mkdir -p "$rel"
  for target in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
    local dir="$WORK/pkg-$target"
    mkdir -p "$dir"
    cp "$fake" "$dir/gry"
    chmod 0755 "$dir/gry"
    tar -czf "$rel/gry-$target.tar.gz" -C "$dir" gry
  done
  (
    cd "$rel"
    if command -v sha256sum >/dev/null 2>&1; then
      sha256sum gry-*.tar.gz >SHA256SUMS
    else
      shasum -a 256 gry-*.tar.gz >SHA256SUMS
    fi
  )
}

case "$(uname -s)" in
  Darwin) case "$(uname -m)" in arm64|aarch64) TEST_TARGET=aarch64-apple-darwin ;; *) TEST_TARGET=x86_64-apple-darwin ;; esac ;;
  *) case "$(uname -m)" in aarch64|arm64) TEST_TARGET=aarch64-unknown-linux-musl ;; *) TEST_TARGET=x86_64-unknown-linux-musl ;; esac ;;
esac

run_install() {
  local home="$1"
  shift
  env HOME="$home" \
    GRY_INSTALL_DIR="${GRY_INSTALL_DIR_OVERRIDE:-$home/bin}" \
    GRY_RELEASES_URL="file://$WORK/rel" \
    GRY_MODIFY_PATH=0 \
    GRY_CLIENT="${GRY_CLIENT_OVERRIDE:-none}" \
    GRY_TEST_LOG="$home/calls.log" \
    PATH="/usr/bin:/bin:/usr/sbin:/sbin" \
    bash "$INSTALLER" "$@"
}

# --- fresh install -----------------------------------------------------------
make_release "$WORK/rel"
H="$WORK/home-fresh"
mkdir -p "$H"
if run_install "$H" >"$H/out" 2>"$H/err"; then
  if [ -x "$H/bin/gry" ] && [ "$("$H/bin/gry" --version)" = "gry 9.9.9" ]; then
    pass "fresh install places an executable gry"
  else
    fail "fresh install binary missing or wrong"
  fi
else
  fail "fresh install failed: $(cat "$H/err")"
fi

# --- repeated install --------------------------------------------------------
if run_install "$H" >"$H/out2" 2>"$H/err2"; then
  pass "repeated install succeeds"
else
  fail "repeated install failed: $(cat "$H/err2")"
fi

# --- status ------------------------------------------------------------------
if run_install "$H" --status >"$H/status" 2>"$H/status.err" &&
  grep -Fq '"version":1' "$H/status"; then
  pass "--status reports the integration manifest"
else
  fail "--status failed: $(cat "$H/status.err" 2>/dev/null || true)"
fi

# --- explicit hooks reach the CLI -------------------------------------------
H2="$WORK/home-hooks"
mkdir -p "$H2"
if GRY_CLIENT_OVERRIDE=opencode run_install "$H2" --hooks >"$H2/out" 2>"$H2/err"; then
  if grep -Fq -- "--hooks" "$H2/calls.log" && grep -Fq -- "--client opencode" "$H2/calls.log"; then
    pass "--hooks is forwarded with the selected client"
  else
    fail "--hooks was not forwarded: $(cat "$H2/calls.log" 2>/dev/null || true)"
  fi
else
  fail "hooks install failed: $(cat "$H2/err")"
fi

# --- hooks without a client is an error -------------------------------------
H3="$WORK/home-none"
mkdir -p "$H3"
if run_install "$H3" --hooks >"$H3/out" 2>"$H3/err"; then
  fail "--hooks with --no-mcp should fail"
else
  pass "--hooks without a client fails explicitly"
fi

# --- checksum mismatch -------------------------------------------------------
H4="$WORK/home-badsum"
mkdir -p "$H4"
printf 'tampered\n' >>"$WORK/rel/latest/download/gry-$TEST_TARGET.tar.gz"
if run_install "$H4" >"$H4/out" 2>"$H4/err"; then
  fail "checksum mismatch must fail"
else
  if [ ! -e "$H4/bin/gry" ] && grep -Eqi 'sha-256|checksum|mismatch' "$H4/err"; then
    pass "checksum mismatch fails before installing"
  else
    fail "checksum failure did not leave the prefix clean"
  fi
fi
# Restore a valid release for later checks.
rm -rf "$WORK/rel"
make_release "$WORK/rel"

# --- archive traversal -------------------------------------------------------
H5="$WORK/home-traversal"
mkdir -p "$H5" "$WORK/evil-src"
printf 'x\n' >"$WORK/evil-src/gry"
python3 - "$WORK/rel/latest/download/gry-$TEST_TARGET.tar.gz" "$WORK/evil-src/gry" <<'PY'
import sys, tarfile
out, source = sys.argv[1:3]
with tarfile.open(out, "w:gz") as archive:
    archive.add(source, arcname="../gry-evil")
    archive.add(source, arcname="gry")
PY
(
  cd "$WORK/rel/latest/download"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "gry-$TEST_TARGET.tar.gz" >SHA256SUMS
  else
    shasum -a 256 "gry-$TEST_TARGET.tar.gz" >SHA256SUMS
  fi
)
if run_install "$H5" >"$H5/out" 2>"$H5/err"; then
  fail "unsafe archive must fail"
else
  if [ ! -e "$WORK/gry-evil" ] && [ ! -e "$H5/bin/gry" ]; then
    pass "unsafe archive is rejected before extraction"
  else
    fail "unsafe archive escaped the extraction directory"
  fi
fi

# Restore a valid release after the traversal fixture.
rm -rf "$WORK/rel"
make_release "$WORK/rel"

# --- prefix containing spaces and Unicode ------------------------------------
H6="$WORK/home space ü"
mkdir -p "$H6"
if GRY_INSTALL_DIR_OVERRIDE="$H6/bin space ü" run_install "$H6" >"$H6/out" 2>"$H6/err"; then
  if [ -x "$H6/bin space ü/gry" ]; then
    pass "prefix with spaces and Unicode works"
  else
    fail "space/Unicode prefix did not receive the binary"
  fi
else
  fail "space/Unicode prefix failed: $(cat "$H6/err")"
fi

# --- uninstall ---------------------------------------------------------------
H7="$WORK/home-uninstall"
mkdir -p "$H7"
run_install "$H7" >"$H7/out" 2>"$H7/err" || true
if run_install "$H7" --uninstall >"$H7/out2" 2>"$H7/err2"; then
  if [ ! -e "$H7/bin/gry" ]; then
    pass "--uninstall removes the binary"
  else
    fail "--uninstall left the binary in place"
  fi
else
  fail "--uninstall failed: $(cat "$H7/err2")"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
