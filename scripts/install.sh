#!/usr/bin/env bash
# next-hunk one-click installer.
#
# Downloads the latest prebuilt binary from GitHub Releases and installs it to a
# sensible bin directory. Prebuilts cover:
#   Linux  x86_64 / aarch64  (static musl)
#   macOS  arm64 / x86_64    (dist profile)
# Falls back to `cargo install` (crates.io, then git) on other platforms.
#
# One-click:
#   curl -fsSL https://github.com/wuxiaobai24/next-hunk/raw/main/scripts/install.sh | bash
#
# Inspect first, then run:
#   curl -fsSL https://github.com/wuxiaobai24/next-hunk/raw/main/scripts/install.sh -o install.sh
#   less install.sh
#   bash install.sh [OPTIONS]
#
# Options:
#   --prefix <dir>        install prefix (default: /usr/local or ~/.local)
#   --bin-dir <dir>       explicit bin directory (overrides --prefix)
#   --version <ver>       pin a version (e.g. v0.2.1 or 0.2.1, default: latest)
#   --as-pager            also configure next-hunk as git's core.pager
#   --no-verify-checksum  skip sha256 verification (not recommended)
#   --force               overwrite an existing next-hunk binary
#   -h, --help            show this help
set -euo pipefail

REPO="wuxiaobai24/next-hunk"
REPO_URL="https://github.com/${REPO}"

# ---- defaults / flag parsing -------------------------------------------------
PREFIX=""
BIN_DIR=""
VERSION=""
AS_PAGER=0
VERIFY_CHECKSUM=1
FORCE=0

usage() {
  cat <<'EOF'
next-hunk installer

Usage:
  curl -fsSL https://github.com/wuxiaobai24/next-hunk/raw/main/scripts/install.sh | bash [OPTIONS]

Options:
  --prefix <dir>        install prefix (default: /usr/local or ~/.local)
  --bin-dir <dir>       explicit bin directory (overrides --prefix)
  --version <ver>       pin a version (e.g. v0.2.1 or 0.2.1, default: latest)
  --as-pager            also configure next-hunk as git's core.pager
  --no-verify-checksum  skip sha256 verification (not recommended)
  --force               overwrite an existing next-hunk binary
  -h, --help            show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix) PREFIX="${2:-}"; shift 2 ;;
    --bin-dir) BIN_DIR="${2:-}"; shift 2 ;;
    --version) VERSION="${2:-}"; shift 2 ;;
    --as-pager) AS_PAGER=1; shift ;;
    --no-verify-checksum) VERIFY_CHECKSUM=0; shift ;;
    --force) FORCE=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

# ---- pretty logging ----------------------------------------------------------
if [[ -t 1 ]]; then
  C_RESET=$'\033[0m'; C_BOLD=$'\033[1m'
  C_GREEN=$'\033[32m'; C_RED=$'\033[31m'; C_YELLOW=$'\033[33m'; C_BLUE=$'\033[34m'
  C_DIM=$'\033[2m'
else
  C_RESET=""; C_BOLD=""; C_GREEN=""; C_RED=""; C_YELLOW=""; C_BLUE=""; C_DIM=""
fi

info() { printf '%sinfo%s %s\n' "${C_BLUE}" "${C_RESET}" "$*"; }
ok()   { printf '%s✓%s %s\n' "${C_GREEN}" "${C_RESET}" "$*"; }
warn() { printf '%s!%s %s\n' "${C_YELLOW}" "${C_RESET}" "$*" >&2; }
die()  { printf '%sx %s%s\n' "${C_BOLD}${C_RED}" "$*" "${C_RESET}" >&2; exit 1; }

# ---- temp dir + cleanup ------------------------------------------------------
TMP="$(mktemp -d)"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

# ---- require tools -----------------------------------------------------------
require() {
  command -v "$1" >/dev/null 2>&1 || die "required tool '$1' not found in PATH; please install it"
}
require curl
require tar

# ---- platform detection ------------------------------------------------------
OS="$(uname -s)"
ARCH="$(uname -m)"

# Resolve the prebuilt asset suffix published by .github/workflows/release.yml.
# Empty string → fall back to cargo install.
prebuilt_target() {
  case "$OS/$ARCH" in
    Linux/x86_64|Linux/amd64) echo "x86_64-musl" ;;
    Linux/aarch64|Linux/arm64) echo "aarch64-musl" ;;
    Darwin/arm64|Darwin/aarch64) echo "aarch64-apple-darwin" ;;
    Darwin/x86_64) echo "x86_64-apple-darwin" ;;
    *) echo "" ;;
  esac
}

# ---- latest version resolution ----------------------------------------------
# Follow the /releases/latest redirect; the effective URL ends with /tag/<tag>.
# Avoids GitHub API rate limits.
resolve_latest_version() {
  local url tag
  url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
          "${REPO_URL}/releases/latest" 2>/dev/null || true)"
  [[ -n "$url" ]] || die "could not resolve latest release; pass --version <ver> to pin"
  tag="${url##*/}"
  [[ -n "$tag" && "$tag" != "latest" ]] || die "could not parse latest tag from '$url'"
  echo "$tag"
}

# Accept both "v0.2.1" and "0.2.1"; return the bare "0.2.1" for asset names.
normalize_version() {
  local v="$1"
  [[ "$v" == v* ]] && v="${v:1}"
  echo "$v"
}

# ---- checksum verification ---------------------------------------------------
verify_checksum() {
  local file="$1" shafile="$2" actual expected
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$file" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  else
    die "neither sha256sum nor shasum is available; cannot verify checksum"
  fi
  expected="$(awk '{print $1}' "$shafile")"
  [[ -n "$expected" ]] || die "could not read expected checksum from $shafile"
  # Lowercase both sides (shasum emits uppercase on some platforms).
  actual="${actual,,}"; expected="${expected,,}"
  if [[ "$actual" != "$expected" ]]; then
    die "checksum mismatch
  expected: $expected
  actual:   $actual"
  fi
  ok "checksum verified"
}

# ---- cargo fallback ----------------------------------------------------------
# Prefer crates.io (`cargo install next-hunk`); fall back to the GitHub repo
# when the registry is unreachable or the version is not published yet.
install_via_cargo() {
  if ! command -v cargo >/dev/null 2>&1; then
    die "no prebuilt binary for ${OS}/${ARCH} and cargo is not installed. \
Install Rust from https://rustup.rs, then run: cargo install next-hunk
(or: cargo install --git ${REPO_URL})"
  fi
  local ver_args=()
  if [[ -n "$VERSION" ]]; then
    local bare
    bare="$(normalize_version "$VERSION")"
    ver_args=(--version "${bare}")
  fi
  # crates.io first (official once published).
  if cargo install next-hunk --locked "${ver_args[@]+"${ver_args[@]}"}" 2>/dev/null; then
    ok "installed next-hunk via cargo (crates.io)"
    return 0
  fi
  # GitHub fallback (works for every historical version / unreleased main).
  info "crates.io install unavailable; running: cargo install --git ${REPO_URL} --locked"
  if [[ -n "$VERSION" ]]; then
    local tag
    tag="v$(normalize_version "$VERSION")"
    cargo install --git "${REPO_URL}" --tag "${tag}" --locked
  else
    cargo install --git "${REPO_URL}" --locked
  fi
  ok "installed next-hunk via cargo (git)"
}

# ---- install dir selection ---------------------------------------------------
pick_install_dir() {
  if [[ -n "$BIN_DIR" ]]; then echo "$BIN_DIR"; return; fi
  if [[ -n "$PREFIX" ]]; then echo "${PREFIX}/bin"; return; fi
  # Prefer /usr/local/bin if writable (covers sudo installs); else ~/.local/bin.
  if [[ -w /usr/local/bin ]]; then echo "/usr/local/bin"; return; fi
  echo "${HOME}/.local/bin"
}

ensure_dir_writable() {
  local d="$1"
  mkdir -p "$d" || die "could not create bin dir: $d"
  [[ -w "$d" ]] || die "bin dir not writable: $d (try --prefix <dir> or run with sudo)"
}

# ---- main --------------------------------------------------------------------
main() {
  info "next-hunk installer"

  local target
  target="$(prebuilt_target)"

  if [[ -z "$target" ]]; then
    # No prebuilt binary for this platform → fall back to cargo.
    info "no prebuilt binary for ${OS}/${ARCH}; trying cargo install"
    install_via_cargo
  else
    # Prebuilt path.
    local tag version asset_url sha_url
    if [[ -n "$VERSION" ]]; then
      version="$(normalize_version "$VERSION")"
      tag="v${version}"
    else
      tag="$(resolve_latest_version)"
      version="$(normalize_version "$tag")"
    fi
    info "target version: ${C_BOLD}${tag}${C_RESET}  ${C_DIM}(${OS}/${ARCH} → ${target})${C_RESET}"

    asset_url="${REPO_URL}/releases/download/${tag}/next-hunk-${version}-${target}.tar.xz"
    sha_url="${asset_url}.sha256"

    local tarball="${TMP}/next-hunk.tar.xz"
    local shafile="${TMP}/next-hunk.tar.xz.sha256"

    info "downloading ${asset_url}"
    if ! curl -fsSL "$asset_url" -o "$tarball"; then
      # Older releases only shipped x86_64-musl. Fall back to cargo for
      # platforms that gained prebuilts after that release, or when the user
      # pins a version that predates multi-arch artifacts.
      warn "no prebuilt asset for ${target} at ${tag}; falling back to cargo install"
      install_via_cargo
      # Optional pager wiring still runs below.
    else
      if (( VERIFY_CHECKSUM )); then
        info "downloading checksum"
        curl -fsSL "$sha_url" -o "$shafile" || die "checksum download failed: $sha_url"
        verify_checksum "$tarball" "$shafile"
      else
        warn "skipping checksum verification (--no-verify-checksum)"
      fi

      info "extracting"
      if ! tar -xJf "$tarball" -C "$TMP" 2>/dev/null; then
        die "extraction failed (tar may lack xz support; install 'xz-utils' / 'xz' and retry)"
      fi
      local srcdir="${TMP}/next-hunk-${version}-${target}"
      local srcbin="${srcdir}/next-hunk"
      [[ -f "$srcbin" ]] || die "extracted binary not found: $srcbin"

      local bin
      bin="$(pick_install_dir)"
      ensure_dir_writable "$bin"
      info "installing to ${C_BOLD}${bin}/next-hunk${C_RESET}"

      if [[ -e "${bin}/next-hunk" && $FORCE -eq 0 ]]; then
        die "existing binary at ${bin}/next-hunk (pass --force to overwrite)"
      fi
      install -m 0755 "$srcbin" "${bin}/next-hunk"

      ok "installed ${bin}/next-hunk"
      "${bin}/next-hunk" --version || true

      # PATH hint for ~/.local/bin.
      case ":${PATH}:" in
        *":${bin}:"*) ;;
        *) printf '%s!%s %s is not on your PATH.\n' "${C_YELLOW}" "${C_RESET}" "$bin"
           printf '   Add it with:  export PATH="%s:\$PATH"\n' "$bin" ;;
      esac
    fi
  fi

  # Optional: wire up as git pager.
  if (( AS_PAGER )); then
    if command -v git >/dev/null 2>&1; then
      git config --global core.pager "next-hunk pager"
      ok "configured git core.pager = next-hunk pager"
    else
      warn "--as-pager given but git is not installed; skipping"
    fi
  fi

  printf '\n%s→ next-hunk installed.%s Quick start:\n' "${C_GREEN}${C_BOLD}" "${C_RESET}"
  printf '   next-hunk                  %s# review the working-tree diff%s\n' "${C_DIM}" "${C_RESET}"
  printf '   next-hunk diff --staged\n'
  printf '   git diff | next-hunk patch -\n'
}

main "$@"
