#!/usr/bin/env bash
# release.test.sh — unit tests for the pure functions in scripts/release.sh.
#
# These cover the parts that are deterministic and safe to test in any
# environment: version normalization/validation, semver compare/bump, the
# Cargo.toml version setter, and (most importantly) the CHANGELOG rotation.
# Git/gh side effects are intentionally NOT tested here.
#
# Run: bash scripts/release.test.sh
# Exits non-zero on the first failure. No external dependencies (no bats).

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RELEASE_SH="$HERE/release.sh"

# Source the script's functions without running its main flow. The script
# early-returns when RELEASE_SH_SOURCED=1.
RELEASE_SH_SOURCED=1
# shellcheck disable=SC1090
source "$RELEASE_SH"
unset RELEASE_SH_SOURCED

# ─────────────────────────────────────────────────────────────────────────────
# Test harness
# ─────────────────────────────────────────────────────────────────────────────
PASS=0
FAIL=0
FAILED_TESTS=()

# assert_eq <name> <expected> <actual>
assert_eq() {
  local name="$1" expected="$2" actual="$3"
  if [[ "$expected" == "$actual" ]]; then
    PASS=$((PASS + 1))
    printf '  ok   %s\n' "$name"
  else
    FAIL=$((FAIL + 1))
    FAILED_TESTS+=("$name")
    printf '  FAIL %s\n' "$name"
    printf '       expected: %q\n' "$expected"
    printf '       actual:   %q\n' "$actual"
  fi
}

# assert_match <name> <regex> <actual>
assert_match() {
  local name="$1" regex="$2" actual="$3"
  if [[ "$actual" =~ $regex ]]; then
    PASS=$((PASS + 1))
    printf '  ok   %s\n' "$name"
  else
    FAIL=$((FAIL + 1))
    FAILED_TESTS+=("$name")
    printf '  FAIL %s — /%s/ did not match\n' "$name" "$regex"
    printf '       actual:   %q\n' "$actual"
  fi
}

# assert_contains <name> <substring> <text>
assert_contains() {
  local name="$1" needle="$2" haystack="$3"
  if [[ "$haystack" == *"$needle"* ]]; then
    PASS=$((PASS + 1))
    printf '  ok   %s\n' "$name"
  else
    FAIL=$((FAIL + 1))
    FAILED_TESTS+=("$name")
    printf '  FAIL %s — substring not found\n' "$name"
    printf '       needle:   %q\n' "$needle"
    printf '       haystack: %q\n' "$haystack"
  fi
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Running release.sh unit tests…"

# ─────────────────────────────────────────────────────────────────────────────
# normalize_version
# ─────────────────────────────────────────────────────────────────────────────
echo "[normalize_version]"
assert_eq "strips leading v"        "0.3.0" "$(normalize_version v0.3.0)"
assert_eq "keeps bare version"      "0.3.0" "$(normalize_version 0.3.0)"

# ─────────────────────────────────────────────────────────────────────────────
# is_valid_version
# ─────────────────────────────────────────────────────────────────────────────
echo "[is_valid_version]"
if is_valid_version 1.2.3; then assert_eq "accepts 1.2.3"    "ok" "ok"; else assert_eq "accepts 1.2.3"    "ok" "NO"; fi
if is_valid_version 1.2.3-beta; then assert_eq "rejects pre-release" "NO" "ok"; else assert_eq "rejects pre-release" "ok" "ok"; fi
if is_valid_version 1.2;     then assert_eq "rejects 1.2"   "NO" "ok"; else assert_eq "rejects 1.2"   "ok" "ok"; fi
if is_valid_version "";      then assert_eq "rejects empty" "NO" "ok"; else assert_eq "rejects empty" "ok" "ok"; fi

# ─────────────────────────────────────────────────────────────────────────────
# semver_cmp
# ─────────────────────────────────────────────────────────────────────────────
echo "[semver_cmp]"
assert_eq "1.0.0 < 1.0.1"  "-1" "$(semver_cmp 1.0.0 1.0.1)"
assert_eq "1.0.1 < 1.1.0"  "-1" "$(semver_cmp 1.0.1 1.1.0)"
assert_eq "1.1.0 < 2.0.0"  "-1" "$(semver_cmp 1.1.0 2.0.0)"
assert_eq "1.0.0 == 1.0.0"  "0" "$(semver_cmp 1.0.0 1.0.0)"
assert_eq "2.0.0 > 1.9.9"  "1" "$(semver_cmp 2.0.0 1.9.9)"

# ─────────────────────────────────────────────────────────────────────────────
# bump_semver
# ─────────────────────────────────────────────────────────────────────────────
echo "[bump_semver]"
assert_eq "patch 1.2.3"   "1.2.4"   "$(bump_semver 1.2.3 patch)"
assert_eq "minor 1.2.3"   "1.3.0"   "$(bump_semver 1.2.3 minor)"
assert_eq "major 1.2.3"   "2.0.0"   "$(bump_semver 1.2.3 major)"
assert_eq "minor zeroes patch" "9.10.0" "$(bump_semver 9.9.7 minor)"

# ─────────────────────────────────────────────────────────────────────────────
# set_cargo_version + cargo_version_clean round-trip
# ─────────────────────────────────────────────────────────────────────────────
echo "[set_cargo_version]"
CARGO_IN="$TMP/Cargo.toml"
cat >"$CARGO_IN" <<'EOF'
[package]
name = "next-hunk"
version = "0.2.1"
edition = "2021"

[lib]
name = "next_hunk"
EOF
assert_eq "reads current version" "0.2.1" "$(cargo_version_clean "$CARGO_IN")"
set_cargo_version "$CARGO_IN" "0.3.0"
assert_eq "writes new version"     "0.3.0" "$(cargo_version_clean "$CARGO_IN")"
# Confirm only the package version line changed, the lib name is intact.
assert_contains "lib name untouched" 'name = "next_hunk"' "$(cat "$CARGO_IN")"
# Confirm it is still the FIRST version line that changed (no other pattern).
assert_contains "edition untouched" 'edition = "2021"' "$(cat "$CARGO_IN")"

# ─────────────────────────────────────────────────────────────────────────────
# bump_changelog — the core transformation
# ─────────────────────────────────────────────────────────────────────────────
echo "[bump_changelog]"

CL_IN="$TMP/CHANGELOG.md"
cat >"$CL_IN" <<'EOF'
# Changelog

All notable changes.

## [Unreleased]

### Added — Widget
- New shiny feature.

### Fixed
- A bug.

## [0.2.1] - 2026-07-13

### Added
- Old release note.

## [0.1.0] - 2026-07-12

First release.

[Unreleased]: https://github.com/wuxiaobai24/next-hunk/compare/v0.1.0...HEAD
[0.2.1]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.2.1
[0.1.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.1.0
EOF

OUT="$(bump_changelog "$CL_IN" "0.3.0" "2026-08-01")"

# 1. Fresh empty Unreleased at the top.
assert_match "empty unreleased section" $'\n## \\[Unreleased\\]\n\n## \\[0\.3\.0\\]' "$OUT"

# 2. New dated version heading present.
assert_contains "new version heading" "## [0.3.0] - 2026-08-01" "$OUT"

# 3. The old Unreleased body moved under the new heading (not duplicated above).
#    Count occurrences of the moved bullet — should appear exactly once.
moved_count="$(printf '%s\n' "$OUT" | grep -c 'New shiny feature.' || true)"
assert_eq "body moved (appears once)" "1" "$moved_count"

# 4. The old released sections are preserved verbatim.
assert_contains "old 0.2.1 heading preserved" "## [0.2.1] - 2026-07-13" "$OUT"
assert_contains "old 0.1.0 heading preserved" "## [0.1.0] - 2026-07-12" "$OUT"

# 5. Link table: [Unreleased] repointed to compare/v0.3.0...HEAD.
assert_contains "unreleased link repointed" \
  "[Unreleased]: https://github.com/wuxiaobai24/next-hunk/compare/v0.3.0...HEAD" "$OUT"

# 6. Link table: new version entry appended.
assert_contains "new version link appended" \
  "[0.3.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.3.0" "$OUT"

# 7. The new-version link appears exactly once (no stale duplicate).
link_count="$(printf '%s\n' "$OUT" | grep -c '^\[0.3.0\]:' || true)"
assert_eq "new version link unique" "1" "$link_count"

# 8. Older link entries preserved.
assert_contains "0.2.1 link preserved" \
  "[0.2.1]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.2.1" "$OUT"

# ─────────────────────────────────────────────────────────────────────────────
# bump_changelog — link-table rebuild: ignores in-prose links, repairs gaps.
# Regression: the repo URL must come from a link-reference line, NOT from an
# inline markdown link to a *different* project. Also fills in any released
# version whose link entry was missing.
# ─────────────────────────────────────────────────────────────────────────────
echo "[bump_changelog / link-table rebuild]"
CL_REBUILD="$TMP/rebuild.md"
cat >"$CL_REBUILD" <<'EOF'
# Changelog

All notable changes. Uses [cargo-husky](https://github.com/rhysd/cargo-husky)).

## [Unreleased]

### Added
- New.

## [0.2.1] - 2026-07-13

### Added
- Mid.

## [0.2.0] - 2026-07-13

### Added
- Older.

## [0.1.0] - 2026-07-12

First.

[Unreleased]: https://github.com/wuxiaobai24/next-hunk/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.1.0
EOF
OUTR="$(bump_changelog "$CL_REBUILD" "0.3.0" "2026-08-01")"

# The repo base must be next-hunk, NOT cargo-husky (the in-prose link).
assert_contains "repo not stolen from prose link" \
  "[Unreleased]: https://github.com/wuxiaobai24/next-hunk/compare/v0.3.0...HEAD" "$OUTR"
assert_contains "version link uses correct repo" \
  "[0.3.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.3.0" "$OUTR"

# Missing entries for 0.2.0 and 0.2.1 must now be present (rebuilt table).
assert_contains "rebuild fills missing 0.2.1" \
  "[0.2.1]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.2.1" "$OUTR"
assert_contains "rebuild fills missing 0.2.0" \
  "[0.2.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.2.0" "$OUTR"

# The in-prose cargo-husky link must survive untouched in the body.
assert_contains "prose link survives" \
  "[cargo-husky](https://github.com/rhysd/cargo-husky))" "$OUTR"

# Exactly one link line per released version + Unreleased (4 versions + 1).
link_lines="$(printf '%s\n' "$OUTR" | grep -cE '^\[[^]]+\]: https://' || true)"
assert_eq "exactly 5 link-reference lines" "5" "$link_lines"

# ─────────────────────────────────────────────────────────────────────────────
# bump_changelog — no link table present (appends a minimal one)
# ─────────────────────────────────────────────────────────────────────────────
echo "[bump_changelog / no link table]"
CL_NOLINK="$TMP/nolink.md"
cat >"$CL_NOLINK" <<'EOF'
# Changelog

## [Unreleased]

### Added
- Something.

## [0.1.0] - 2026-07-12

First.
EOF
OUT2="$(bump_changelog "$CL_NOLINK" "0.2.0" "2026-08-01")"
assert_contains "creates unreleased link" \
  "[Unreleased]: https://github.com/wuxiaobai24/next-hunk/compare/v0.2.0...HEAD" "$OUT2"
assert_contains "creates version link" \
  "[0.2.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.2.0" "$OUT2"

# ─────────────────────────────────────────────────────────────────────────────
# bump_changelog — empty Unreleased body (idempotent-ish: still adds heading)
# ─────────────────────────────────────────────────────────────────────────────
echo "[bump_changelog / empty unreleased]"
CL_EMPTY="$TMP/empty.md"
cat >"$CL_EMPTY" <<'EOF'
# Changelog

## [Unreleased]

## [0.1.0] - 2026-07-12

First.

[Unreleased]: https://github.com/wuxiaobai24/next-hunk/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/wuxiaobai24/next-hunk/releases/tag/v0.1.0
EOF
OUT3="$(bump_changelog "$CL_EMPTY" "0.2.0" "2026-08-01")"
assert_contains "still adds new version heading" "## [0.2.0] - 2026-08-01" "$OUT3"
# Two version headings + Unreleased, no garbage blanks duplicated.
hd_count="$(printf '%s\n' "$OUT3" | grep -c '^## \[' || true)"
assert_eq "exactly three headings" "3" "$hd_count"

# ─────────────────────────────────────────────────────────────────────────────
# Summary
# ─────────────────────────────────────────────────────────────────────────────
echo
if [[ "$FAIL" -eq 0 ]]; then
  printf 'PASS: all %d assertions passed.\n' "$PASS"
  exit 0
else
  printf 'FAIL: %d assertion(s) failed, %d passed.\n' "$FAIL" "$PASS"
  printf 'Failed: %s\n' "${FAILED_TESTS[*]}"
  exit 1
fi
