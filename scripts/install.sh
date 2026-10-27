#!/usr/bin/env bash
#
# gry installer — local-first code intelligence for AI agents.
#
# Canonical install:
#   curl -fsSL https://github.com/PizenLabs/gry/releases/latest/download/install.sh | bash
#
# Development install:
#   curl -fsSL https://raw.githubusercontent.com/PizenLabs/gry/main/scripts/install.sh | bash
#
# The installer:
#   1. detects macOS/Linux and the machine architecture (including Rosetta);
#   2. downloads the matching prebuilt binary and verifies its SHA-256;
#   3. falls back to a Cargo source build when no asset matches;
#   4. installs `gry` into ~/.local/bin (or /usr/local/bin as root);
#   5. detects installed agent harnesses and configures their MCP entry with
#      `gry install --client auto`;
#   6. optionally installs the gry agent skill and, with --hooks, the OpenCode
#      V2 lifecycle plugin.
#
# Environment:
#   GRY_VERSION          Release tag to install (default: latest).
#   GRY_INSTALL_DIR      Binary directory (default: ~/.local/bin).
#   GRY_CLIENT           auto, all, none, or a client id (default: auto).
#   GRY_REPO             GitHub owner/repo (default: PizenLabs/gry).
#   GRY_REPO_URL         Override the source-build Git URL.
#   GRY_RELEASES_URL     Override release base URL (mirrors/testing).
#   GRY_MODIFY_PATH      1 or 0; default 1 in non-interactive mode.
#
# Usage: install.sh [options]

set -euo pipefail

REPO="${GRY_REPO:-PizenLabs/gry}"
REPO_URL="${GRY_REPO_URL:-https://github.com/${REPO}.git}"
RELEASES_URL="${GRY_RELEASES_URL:-https://github.com/${REPO}/releases}"
VERSION="${GRY_VERSION:-latest}"
PREFIX="${GRY_INSTALL_DIR:-}"
CLIENT="${GRY_CLIENT:-auto}"
MODIFY_PATH="${GRY_MODIFY_PATH:-}"
SKILL=1
HOOKS=0
STATUS=0
INDEX_PATH=""
FROM_SOURCE=0
FORCE=0
DRY_RUN=0
INSECURE=0
ASSUME_YES=0
INTERACTIVE=0
QUIET=0
UNINSTALL=0
SETUP_FAILED=0
BIN=""

if [ -t 2 ]; then
  C_RESET="$(printf '\033[0m')"
  C_BOLD="$(printf '\033[1m')"
  C_DIM="$(printf '\033[2m')"
else
  C_RESET=""
  C_BOLD=""
  C_DIM=""
fi

log() {
  if [ "$QUIET" -eq 0 ]; then
    printf '%s%s%s\n' "$C_DIM" "$*" "$C_RESET" >&2
  fi
}

say() {
  if [ "$QUIET" -eq 0 ]; then
    printf '%s\n' "$*" >&2
  fi
}

warn() {
  printf 'gry installer: warning: %s\n' "$*" >&2
}

die() {
  printf 'gry installer: error: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
gry installer

Usage:
  install.sh [options]

Options:
  --version <tag|latest>   Release to install (default: latest)
  --prefix <dir>           Install directory (default: ~/.local/bin)
  --client <id|auto|all|none>
                           Harnesses to configure (default: auto)
                           Comma-separated ids are accepted: claude,opencode
  --from-source            Build with Cargo instead of downloading a binary
  --index [path]           Index a repository after setup (default: .)
  --skill                  Install the gry agent skill (default)
  --no-skill               Do not install the agent skill
  --hooks                  Install native lifecycle hooks where supported
                           (currently the OpenCode V2 plugin). Opt-in.
  --status                 Print the managed integration status and exit
  --no-mcp                 Skip MCP configuration
  --modify-path            Add the install directory to the shell PATH
  --no-modify-path         Do not edit shell startup files
  --interactive            Ask before changing MCP/PATH settings
  --force                  Reinstall even when the version already matches
  --dry-run                Print the plan; do not change the system
  --insecure               Skip SHA-256 verification
  --uninstall              Remove gry, MCP entries, and the PATH block
  -y, --yes                Assume yes for prompts
  -q, --quiet              Errors only
  -h, --help               Show this help

Examples:
  curl -fsSL https://github.com/PizenLabs/gry/releases/latest/download/install.sh | bash
  curl -fsSL .../install.sh | bash -s -- --client claude,opencode
  curl -fsSL .../install.sh | bash -s -- --version v1.0.0 --prefix ~/bin
  curl -fsSL .../install.sh | bash -s -- --uninstall

Detected clients: claude, claude-desktop, cursor, windsurf, gemini, opencode, omp,
ohgo, codex, zed, cline, roo, kilo, vscode, trae, qwen, augment, amp.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      [ "$#" -ge 2 ] || die "--version requires a value"
      VERSION="$2"
      shift 2
      ;;
    --version=*)
      VERSION="${1#*=}"
      shift
      ;;
    --prefix)
      [ "$#" -ge 2 ] || die "--prefix requires a value"
      PREFIX="$2"
      shift 2
      ;;
    --prefix=*)
      PREFIX="${1#*=}"
      shift
      ;;
    --client)
      [ "$#" -ge 2 ] || die "--client requires a value"
      CLIENT="$2"
      shift 2
      ;;
    --client=*)
      CLIENT="${1#*=}"
      shift
      ;;
    --from-source)
      FROM_SOURCE=1
      shift
      ;;
    --index)
      INDEX_PATH="."
      shift
      if [ "$#" -gt 0 ]; then
        case "$1" in
          -*) ;;
          *)
            INDEX_PATH="$1"
            shift
            ;;
        esac
      fi
      ;;
    --index=*)
      INDEX_PATH="${1#*=}"
      shift
      ;;
    --skill)
      SKILL=1
      shift
      ;;
    --no-skill)
      SKILL=0
      shift
      ;;
    --hooks)
      HOOKS=1
      shift
      ;;
    --status)
      STATUS=1
      shift
      ;;
    --no-mcp)
      CLIENT="none"
      shift
      ;;
    --modify-path)
      MODIFY_PATH=1
      shift
      ;;
    --no-modify-path)
      MODIFY_PATH=0
      shift
      ;;
    --interactive)
      INTERACTIVE=1
      shift
      ;;
    --force)
      FORCE=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --insecure)
      INSECURE=1
      shift
      ;;
    --uninstall)
      UNINSTALL=1
      shift
      ;;
    -y|--yes)
      ASSUME_YES=1
      shift
      ;;
    -q|--quiet)
      QUIET=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1 (try --help)"
      ;;
  esac
done

# Normalize the release tag. GitHub tags are v-prefixed.
case "$VERSION" in
  latest|"") VERSION="latest" ;;
  v*) ;;
  *) VERSION="v$VERSION" ;;
esac

# Expand a leading ~ and remove a trailing slash.
expand_prefix() {
  case "$PREFIX" in
    "~") PREFIX="$HOME" ;;
    "~/"*) PREFIX="$HOME/${PREFIX#~/}" ;;
  esac
  while [ "${PREFIX%/}" != "$PREFIX" ]; do
    PREFIX="${PREFIX%/}"
  done
}

if [ -z "$PREFIX" ]; then
  if [ "$(id -u)" -eq 0 ]; then
    PREFIX="/usr/local/bin"
  else
    PREFIX="$HOME/.local/bin"
  fi
fi
expand_prefix
BIN="$PREFIX/gry"
[ -n "$CLIENT" ] || CLIENT="auto"

case "$MODIFY_PATH" in
  0|false|no) MODIFY_PATH=0 ;;
  1|true|yes) MODIFY_PATH=1 ;;
  "")
    if [ "$INTERACTIVE" -eq 1 ] && [ "$ASSUME_YES" -eq 0 ]; then
      MODIFY_PATH="ask"
    else
      MODIFY_PATH=1
    fi
    ;;
  *) MODIFY_PATH=1 ;;
esac

TMP=""
cleanup() {
  if [ -n "$TMP" ] && [ -d "$TMP" ]; then
    rm -rf "$TMP"
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

need_tmp() {
  if [ -z "$TMP" ]; then
    TMP="$(mktemp -d "${TMPDIR:-/tmp}/gry-install.XXXXXX")" || die "cannot create a temporary directory"
  fi
}

fetch() {
  url="$1"
  dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 3 --retry-delay 1 --connect-timeout 15 --max-time 1800 \
      -o "$dest" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$dest" "$url"
  else
    die "curl or wget is required to download gry"
  fi
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    die "sha256sum or shasum is required to verify the download"
  fi
}

command_exists() {
  command -v "$1" >/dev/null 2>&1
}

detect_platform() {
  os="$(uname -s 2>/dev/null || echo unknown)"
  arch="$(uname -m 2>/dev/null || echo unknown)"
  TARGET=""
  TARGETS=""
  PLATFORM_LABEL="$os/$arch"

  case "$os" in
    Darwin)
      case "$arch" in
        arm64|aarch64)
          TARGET="aarch64-apple-darwin"
          ;;
        x86_64)
          # A shell running under Rosetta reports x86_64. Prefer the native
          # arm64 asset when the machine is Apple Silicon.
          if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
            TARGET="aarch64-apple-darwin"
          else
            TARGET="x86_64-apple-darwin"
          fi
          ;;
        *)
          die "unsupported macOS architecture: $arch"
          ;;
      esac
      TARGETS="$TARGET"
      ;;
    Linux)
      case "$arch" in
        x86_64|amd64)
          arch="x86_64"
          ;;
        aarch64|arm64)
          arch="aarch64"
          ;;
        armv7l|armv7)
          arch="armv7"
          ;;
        i386|i686)
          arch="i686"
          ;;
      esac
      libc="gnu"
      if [ -f /etc/alpine-release ] || ldd --version 2>&1 | grep -qi musl; then
        libc="musl"
      fi
      # Musl builds are static and run on both glibc and musl systems, so
      # prefer musl and fall back to the matching gnu asset.
      case "$arch" in
        x86_64)
          TARGET="x86_64-unknown-linux-musl"
          TARGETS="x86_64-unknown-linux-musl x86_64-unknown-linux-gnu"
          ;;
        aarch64)
          TARGET="aarch64-unknown-linux-musl"
          TARGETS="aarch64-unknown-linux-musl aarch64-unknown-linux-gnu"
          ;;
        *)
          target_libc="$libc"
          TARGETS="${arch}-unknown-linux-${target_libc}"
          ;;
      esac
      ;;
    MINGW*|MSYS*|CYGWIN*)
      die "Windows is not supported by this installer; use WSL or build from source"
      ;;
    *)
      if [ "$FROM_SOURCE" -eq 0 ]; then
        die "unsupported operating system: $os; re-run with --from-source"
      fi
      ;;
  esac
  log "platform: $PLATFORM_LABEL (target: ${TARGET:-none})"
}

install_binary() {
  src="$1"
  [ -f "$src" ] || die "binary not found: $src"
  if [ ! -d "$PREFIX" ]; then
    mkdir -p "$PREFIX" 2>/dev/null || die "cannot create $PREFIX"
  fi
  if [ ! -w "$PREFIX" ]; then
    die "cannot write to $PREFIX; re-run with sudo or --prefix \"\$HOME/.local/bin\""
  fi
  dest="$PREFIX/.gry.tmp.$$"
  cp "$src" "$dest" || die "cannot copy the gry binary into $PREFIX"
  chmod 0755 "$dest" || die "cannot make $dest executable"
  mv -f "$dest" "$BIN" || die "cannot move the gry binary into place"
  log "installed $BIN"
}

try_prebuilt() {
  target="$1"
  asset="gry-${target}.tar.gz"
  if [ "$VERSION" = "latest" ]; then
    base="$RELEASES_URL/latest/download"
  else
    base="$RELEASES_URL/download/$VERSION"
  fi
  url="$base/$asset"
  log "downloading $url"
  if ! fetch "$url" "$TMP/$asset"; then
    warn "no prebuilt binary for $target"
    return 1
  fi
  if [ "$INSECURE" -eq 0 ]; then
    if ! fetch "$base/SHA256SUMS" "$TMP/SHA256SUMS"; then
      die "SHA256SUMS is unavailable for $VERSION; re-run with --insecure to skip verification"
    fi
    expected="$(awk -v asset="$asset" '$2 == asset {print $1}' "$TMP/SHA256SUMS")"
    if [ -z "$expected" ]; then
      die "SHA256SUMS does not list $asset; re-run with --insecure to skip verification"
    fi
    actual="$(sha256_of "$TMP/$asset")"
    if [ "$expected" != "$actual" ]; then
      die "SHA-256 mismatch for $asset (expected $expected, got $actual)"
    fi
    log "checksum verified"
  fi
  # Reject absolute paths and parent traversal before extracting. An archive
  # is untrusted input even when it came from the project's own release page.
  if tar -tzf "$TMP/$asset" | grep -Eq '(^/|(^|/)\.\.($|/))'; then
    die "refusing to extract $asset: archive contains an unsafe path"
  fi
  mkdir -p "$TMP/extract"
  if ! tar -xzf "$TMP/$asset" -C "$TMP/extract"; then
    warn "could not unpack $asset"
    return 1
  fi
  if [ -L "$TMP/extract/gry" ] || [ ! -f "$TMP/extract/gry" ]; then
    warn "$asset did not contain a regular gry binary"
    return 1
  fi
  install_binary "$TMP/extract/gry"
  return 0
}

build_from_source() {
  if ! command_exists cargo; then
    die "no prebuilt binary for ${TARGET:-this machine} and cargo is not installed; install Rust 1.85+ from https://rustup.rs and re-run, or use a release asset"
  fi
  need_tmp
  log "building gry from source (this can take several minutes)"
  root="$TMP/cargo"
  if [ "$VERSION" = "latest" ]; then
    cargo install --git "$REPO_URL" --locked --root "$root" gry
  else
    cargo install --git "$REPO_URL" --locked --tag "$VERSION" --root "$root" gry
  fi
  [ -x "$root/bin/gry" ] || die "source build did not produce a gry binary"
  install_binary "$root/bin/gry"
}

path_contains_prefix() {
  case ":$PATH:" in
    *":$PREFIX:"*) return 0 ;;
    *) return 1 ;;
  esac
}

shell_rc() {
  shell_name="$(basename "${SHELL:-sh}")"
  case "$shell_name" in
    zsh)
      printf '%s' "$HOME/.zshrc"
      ;;
    bash)
      if [ -f "$HOME/.bash_profile" ]; then
        printf '%s' "$HOME/.bash_profile"
      elif [ -f "$HOME/.bashrc" ]; then
        printf '%s' "$HOME/.bashrc"
      else
        printf '%s' "$HOME/.bash_profile"
      fi
      ;;
    fish)
      printf '%s' "$HOME/.config/fish/config.fish"
      ;;
    *)
      printf '%s' "$HOME/.profile"
      ;;
  esac
}

add_path_block() {
  rc="$(shell_rc)"
  rc_dir="$(dirname "$rc")"
  if [ -f "$rc" ] && grep -Fq "# >>> gry installer >>>" "$rc"; then
    log "PATH already configured in $rc"
    return 0
  fi
  if [ "$DRY_RUN" -eq 1 ]; then
    say "would add $PREFIX to PATH in $rc"
    return 0
  fi
  if ! mkdir -p "$rc_dir" 2>/dev/null; then
    warn "cannot create $rc_dir; add $PREFIX to PATH manually"
    return 0
  fi
  case "$rc" in
    *fish*)
      {
        printf '\n# >>> gry installer >>>\n'
        printf 'fish_add_path "%s"\n' "$PREFIX"
        printf '# <<< gry installer <<<\n'
      } >>"$rc" || warn "cannot update $rc; add $PREFIX to PATH manually"
      ;;
    *)
      {
        printf '\n# >>> gry installer >>>\n'
        printf 'export PATH="%s:$PATH"\n' "$PREFIX"
        printf '# <<< gry installer <<<\n'
      } >>"$rc" || warn "cannot update $rc; add $PREFIX to PATH manually"
      ;;
  esac
  log "added $PREFIX to PATH in $rc"
}

remove_path_blocks() {
  for rc in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.profile" "$HOME/.config/fish/config.fish"; do
    [ -f "$rc" ] || continue
    grep -Fq "# >>> gry installer >>>" "$rc" || continue
    if [ "$DRY_RUN" -eq 1 ]; then
      say "would remove the gry PATH block from $rc"
      continue
    fi
    tmp_rc="$rc.gry.$$"
    if awk '
      /# >>> gry installer >>>/ { skip = 1; next }
      /# <<< gry installer <<</ { skip = 0; next }
      skip != 1 { print }
    ' "$rc" >"$tmp_rc"; then
      mv -f "$tmp_rc" "$rc"
      log "removed the gry PATH block from $rc"
    else
      rm -f "$tmp_rc"
      warn "could not update $rc"
    fi
  done
}

setup_path() {
  if path_contains_prefix; then
    return 0
  fi
  if [ "$MODIFY_PATH" = "ask" ]; then
    printf '%s is not on PATH. Add it to your shell startup file? [Y/n] ' "$PREFIX" >/dev/tty
    answer=""
    read -r answer </dev/tty || answer="y"
    case "$answer" in
      n|N|no|NO)
        say "add this to your shell profile: export PATH=\"$PREFIX:\$PATH\""
        return 0
        ;;
    esac
  elif [ "$MODIFY_PATH" -eq 0 ]; then
    say "add this to your shell profile: export PATH=\"$PREFIX:\$PATH\""
    return 0
  fi
  add_path_block
}

run_install_cmd() {
  if [ "$QUIET" -eq 1 ]; then
    "$BIN" --quiet install "$@"
  else
    "$BIN" install "$@"
  fi
}

run_uninstall_cmd() {
  if [ "$QUIET" -eq 1 ]; then
    "$BIN" --quiet uninstall "$@"
  else
    "$BIN" uninstall "$@"
  fi
}

configure_one_client() {
  client="$1"
  if [ "$DRY_RUN" -eq 1 ]; then
    if [ "$SKILL" -eq 1 ]; then
      if [ "$HOOKS" -eq 1 ]; then
        run_install_cmd --client "$client" --dry-run --skill --hooks
      else
        run_install_cmd --client "$client" --dry-run --skill
      fi
    else
      if [ "$HOOKS" -eq 1 ]; then
        run_install_cmd --client "$client" --dry-run --hooks
      else
        run_install_cmd --client "$client" --dry-run
      fi
    fi
  else
    if [ "$SKILL" -eq 1 ]; then
      if [ "$HOOKS" -eq 1 ]; then
        run_install_cmd --client "$client" --skill --hooks
      else
        run_install_cmd --client "$client" --skill
      fi
    else
      if [ "$HOOKS" -eq 1 ]; then
        run_install_cmd --client "$client" --hooks
      else
        run_install_cmd --client "$client"
      fi
    fi
  fi
}

setup_mcp() {
  if [ "$CLIENT" = "none" ]; then
    if [ "$HOOKS" -eq 1 ]; then
      warn "--hooks requires a client; integration hooks were not installed"
      SETUP_FAILED=1
    fi
    log "skipping MCP configuration"
    return 0
  fi
  [ -x "$BIN" ] || die "gry binary is missing; MCP configuration skipped"

  if [ "$INTERACTIVE" -eq 1 ] && [ "$ASSUME_YES" -eq 0 ]; then
    printf 'Configure MCP for detected agent harnesses? [Y/n] ' >/dev/tty
    answer=""
    read -r answer </dev/tty || answer="y"
    case "$answer" in
      n|N|no|NO)
        log "skipping MCP configuration"
        return 0
        ;;
    esac
  fi

  log "configuring MCP (client: $CLIENT)"
  case "$CLIENT" in
    auto|all|none)
      configure_one_client "$CLIENT" || SETUP_FAILED=1
      ;;
    *,*)
      old_ifs="$IFS"
      IFS=','
      for client in $CLIENT; do
        client="$(printf '%s' "$client" | tr -d '[:space:]')"
        [ -n "$client" ] || continue
        configure_one_client "$client" || SETUP_FAILED=1
      done
      IFS="$old_ifs"
      ;;
    *)
      configure_one_client "$CLIENT" || SETUP_FAILED=1
      ;;
  esac
}

verify_install() {
  [ -x "$BIN" ] || die "gry was not installed at $BIN"
  version_output="$("$BIN" --version 2>/dev/null | head -n 1 || true)"
  [ -n "$version_output" ] || die "$BIN is not runnable"
  say "${C_BOLD}${version_output}${C_RESET} installed at $BIN"
  if ! "$BIN" --quiet doctor >/dev/null 2>&1; then
    warn "gry doctor reported an issue; run: $BIN doctor"
  fi
}

uninstall_one_client() {
  if [ "$DRY_RUN" -eq 1 ]; then
    run_uninstall_cmd --client "$1" --dry-run
  else
    run_uninstall_cmd --client "$1"
  fi
}

uninstall() {
  say "uninstalling gry"
  if [ -x "$BIN" ]; then
    if [ "$CLIENT" != "none" ]; then
      log "removing MCP entries (client: $CLIENT)"
      case "$CLIENT" in
        auto|all|none)
          uninstall_one_client "$CLIENT" || SETUP_FAILED=1
          ;;
        *,*)
          old_ifs="$IFS"
          IFS=','
          for client in $CLIENT; do
            client="$(printf '%s' "$client" | tr -d '[:space:]')"
            [ -n "$client" ] || continue
            uninstall_one_client "$client" || SETUP_FAILED=1
          done
          IFS="$old_ifs"
          ;;
        *)
          uninstall_one_client "$CLIENT" || SETUP_FAILED=1
          ;;
      esac
    fi
  elif [ "$CLIENT" != "none" ]; then
    warn "gry is not installed at $BIN; skipping MCP cleanup"
  fi
  if [ "$DRY_RUN" -eq 1 ]; then
    say "would remove $BIN"
  else
    rm -f "$BIN"
    log "removed $BIN"
  fi
  remove_path_blocks
  if [ "$SETUP_FAILED" -ne 0 ]; then
    die "uninstall completed with errors"
  fi
  say "gry uninstalled"
}

if [ "$STATUS" -eq 1 ]; then
  if [ ! -x "$BIN" ]; then
    die "gry is not installed at $BIN"
  fi
  "$BIN" integrations --verify
  exit 0
fi

if [ "$UNINSTALL" -eq 1 ]; then
  uninstall
  exit 0
fi

detect_platform

if [ "$DRY_RUN" -eq 1 ]; then
  say "dry run: would install $VERSION for ${TARGET:-source build} to $BIN"
  say "dry run: MCP client selection: $CLIENT"
  if [ "$CLIENT" != "none" ] && [ -x "$BIN" ]; then
    if [ "$SKILL" -eq 1 ]; then
      run_install_cmd --client "$CLIENT" --dry-run --skill || true
    else
      run_install_cmd --client "$CLIENT" --dry-run || true
    fi
  fi
  exit 0
fi

need_tmp

# Reuse an exactly pinned installation when it is already present.
skip_install=0
if [ -x "$BIN" ] && [ "$FORCE" -eq 0 ] && [ "$VERSION" != "latest" ]; then
  installed_version="$("$BIN" --version 2>/dev/null | awk '{print $2}' | head -n 1 || true)"
  wanted_version="${VERSION#v}"
  if [ "$installed_version" = "$wanted_version" ]; then
    say "gry $wanted_version is already installed at $BIN"
    skip_install=1
  fi
fi

if [ "$skip_install" -eq 0 ]; then
  installed=0
  if [ "$FROM_SOURCE" -eq 0 ]; then
    for target in $TARGETS; do
      if try_prebuilt "$target"; then
        installed=1
        break
      fi
    done
  fi
  if [ "$installed" -eq 0 ]; then
    build_from_source
  fi
fi

verify_install
setup_path
setup_mcp

if [ -n "$INDEX_PATH" ]; then
  log "indexing $INDEX_PATH"
  "$BIN" index "$INDEX_PATH" || warn "indexing failed; run: $BIN index $INDEX_PATH"
fi

if [ "$SETUP_FAILED" -ne 0 ]; then
  die "gry was installed, but one or more harness configurations failed"
fi

say ""
say "${C_BOLD}gry is ready.${C_RESET}"
if [ "$CLIENT" = "none" ]; then
  say "MCP configuration was skipped. Configure a harness with:"
  say "  gry install --client auto"
else
  say "MCP setup finished. Restart your agent harness(es) to load the gry MCP server."
  say "If no harness was detected, configure one with: gry install --client <id>"
fi
if [ -n "$INDEX_PATH" ]; then
  say "Index another repository with: gry index /path/to/repo"
fi
