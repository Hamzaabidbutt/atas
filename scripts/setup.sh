#!/usr/bin/env bash
#
# One-shot setup for the ATAS desktop app.
#
# Installs the toolchains and system libraries Tauri needs, pulls down
# dependencies, and optionally launches the app. Safe to re-run: every step
# checks whether it is already satisfied before doing anything.
#
#   ./scripts/setup.sh          # set up, then ask before launching
#   ./scripts/setup.sh --run    # set up and launch without asking
#   ./scripts/setup.sh --check  # report what is missing, change nothing
#
set -euo pipefail

RUN_AFTER=0
CHECK_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --run) RUN_AFTER=1 ;;
    --check) CHECK_ONLY=1 ;;
    -h|--help)
      sed -n '3,12p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

bold() { printf '\033[1m%s\033[0m\n' "$1"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$1"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$1"; }
step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Only use sudo when we are not already root and it exists.
SUDO=""
if [ "$(id -u)" -ne 0 ]; then
  if command -v sudo >/dev/null 2>&1; then
    SUDO="sudo"
  else
    warn "not root and sudo is unavailable; system packages cannot be installed"
  fi
fi

MISSING=()

# --- Platform -------------------------------------------------------------
OS="$(uname -s)"
DISTRO=""
if [ "$OS" = "Linux" ] && [ -r /etc/os-release ]; then
  # shellcheck disable=SC1091
  DISTRO="$(. /etc/os-release && echo "${ID_LIKE:-$ID}")"
fi

step "Platform"
case "$OS" in
  Linux)  ok "Linux (${DISTRO:-unknown})" ;;
  Darwin) ok "macOS" ;;
  *)      warn "$OS is not handled by this script; see the README" ;;
esac

# --- System libraries -----------------------------------------------------
step "System libraries"
install_linux_deps() {
  case "$DISTRO" in
    *debian*|*ubuntu*)
      $SUDO apt-get update -qq
      $SUDO apt-get install -y \
        libgtk-3-dev libwebkit2gtk-4.1-dev libsoup-3.0-dev \
        libjavascriptcoregtk-4.1-dev librsvg2-dev patchelf \
        build-essential curl file pkg-config
      ;;
    *fedora*|*rhel*)
      $SUDO dnf install -y gtk3-devel webkit2gtk4.1-devel libsoup3-devel \
        librsvg2-devel patchelf @development-tools
      ;;
    *arch*)
      $SUDO pacman -S --needed --noconfirm webkit2gtk-4.1 gtk3 libsoup3 \
        librsvg patchelf base-devel
      ;;
    *)
      warn "unrecognised distribution; install the Tauri prerequisites manually"
      warn "see https://tauri.app/start/prerequisites/"
      return 1
      ;;
  esac
}

if [ "$OS" = "Linux" ]; then
  if pkg-config --exists webkit2gtk-4.1 2>/dev/null; then
    ok "webkit2gtk-4.1 present"
  else
    MISSING+=("system libraries")
    if [ "$CHECK_ONLY" -eq 1 ]; then
      warn "webkit2gtk-4.1 missing"
    elif [ -z "$SUDO" ] && [ "$(id -u)" -ne 0 ]; then
      warn "cannot install without root"
    else
      echo "  installing (this needs root)…"
      install_linux_deps && ok "installed" || warn "install failed"
    fi
  fi
elif [ "$OS" = "Darwin" ]; then
  if xcode-select -p >/dev/null 2>&1; then
    ok "Xcode command line tools present"
  else
    MISSING+=("Xcode command line tools")
    if [ "$CHECK_ONLY" -eq 0 ]; then
      echo "  a dialog will open; rerun this script once it finishes"
      xcode-select --install || true
    fi
  fi
fi

# --- Rust -----------------------------------------------------------------
step "Rust"
MIN_RUST_MINOR=82
rust_is_new_enough() {
  local v minor
  v="$(rustc --version 2>/dev/null | awk '{print $2}')" || return 1
  minor="$(echo "$v" | cut -d. -f2)"
  [ "${minor:-0}" -ge "$MIN_RUST_MINOR" ]
}

if command -v rustc >/dev/null 2>&1 && rust_is_new_enough; then
  ok "$(rustc --version)"
elif command -v rustup >/dev/null 2>&1; then
  if [ "$CHECK_ONLY" -eq 1 ]; then
    MISSING+=("rust >= 1.$MIN_RUST_MINOR")
    warn "rust is too old: $(rustc --version 2>/dev/null || echo 'not installed')"
  else
    echo "  updating via rustup…"
    rustup update stable && rustup default stable
    ok "$(rustc --version)"
  fi
else
  MISSING+=("rust")
  if [ "$CHECK_ONLY" -eq 1 ]; then
    warn "rust not installed"
  else
    echo "  installing rustup…"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
    ok "$(rustc --version)"
  fi
fi

# --- Node -----------------------------------------------------------------
step "Node"
MIN_NODE_MAJOR=20
if command -v node >/dev/null 2>&1; then
  NODE_MAJOR="$(node --version | sed 's/^v//' | cut -d. -f1)"
  if [ "$NODE_MAJOR" -ge "$MIN_NODE_MAJOR" ]; then
    ok "$(node --version)"
  else
    MISSING+=("node >= $MIN_NODE_MAJOR")
    warn "node $(node --version) is too old; install $MIN_NODE_MAJOR+ from https://nodejs.org"
  fi
else
  MISSING+=("node")
  warn "node not installed; get $MIN_NODE_MAJOR+ from https://nodejs.org"
fi

if [ "$CHECK_ONLY" -eq 1 ]; then
  step "Result"
  if [ ${#MISSING[@]} -eq 0 ]; then
    ok "everything needed is present"
  else
    warn "missing: ${MISSING[*]}"
    exit 1
  fi
  exit 0
fi

if [ ${#MISSING[@]} -ne 0 ]; then
  step "Cannot continue"
  warn "still missing: ${MISSING[*]}"
  warn "resolve the above, then run this script again"
  exit 1
fi

# --- Dependencies ---------------------------------------------------------
step "Project dependencies"
npm install --no-audit --no-fund
ok "Tauri CLI"
npm --prefix ui install --no-audit --no-fund
ok "UI packages"

# --- Sanity check ---------------------------------------------------------
step "Checking the engine"
# Sum the per-binary result lines rather than showing the last one, which is
# usually an empty doc-test target and reads as "0 passed".
TEST_LOG="$(mktemp)"
if cargo test --workspace --quiet >"$TEST_LOG" 2>&1; then
  TOTAL="$(grep -Eo 'test result: ok\. [0-9]+ passed' "$TEST_LOG" \
            | awk '{s+=$4} END {print s+0}')"
  ok "$TOTAL engine tests pass"
else
  warn "engine tests failed - the app may still run, but something is wrong"
  tail -20 "$TEST_LOG"
fi
rm -f "$TEST_LOG"

step "Ready"
cat <<'MSG'
  npm run dev                     launch the app (first build takes 3-6 minutes)
  npm run build -- --no-bundle    build a standalone binary
  cargo test --workspace          run the engine test suite

  The app starts on a built-in synthetic feed: no network or exchange
  account is needed, and everything on screen is simulated.
MSG

if [ "$RUN_AFTER" -eq 1 ]; then
  step "Launching"
  exec npm run dev
fi

printf '\n'
read -r -p "Launch it now? [y/N] " reply
case "$reply" in
  [yY]*) exec npm run dev ;;
  *) echo "Run 'npm run dev' when you are ready." ;;
esac
