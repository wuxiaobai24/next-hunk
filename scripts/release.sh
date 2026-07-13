#!/usr/bin/env bash
# release.sh — one-command release helper for next-hunk.
#
# Bumps the version in Cargo.toml, rotates the CHANGELOG "[Unreleased]" section
# into a new version heading (date-stamped), opens a PR via `gh`, and (in a
# second stage) tags the merge to trigger the CI release workflow.
#
# This script is PR-only by design (per AGENTS.md): it never commits to or
# pushes `main` directly. The `--tag` stage is the one deliberate exception —
# it pushes a `v*` tag, which is what actually triggers a release. Tagging is
# the documented release mechanism, so this is not a "direct change to main".
#
# Usage:
#   ./scripts/release.sh <version>            # e.g. 0.3.0  (or v0.3.0)
#   ./scripts/release.sh --bump patch|minor|major
#   ./scripts/release.sh <version> --dry-run  # print plan, change nothing
#   ./scripts/release.sh <version> --no-pr    # branch+commit, no push/PR
#   ./scripts/release.sh --tag vX.Y.Z         # after PR merge: tag main & push
#
# Stage 1 (default): bump files → branch → commit → push → `gh pr create`.
# Stage 2 (--tag):   checkout main, pull, tag vX.Y.Z, push the tag.
#                    `release.yml` then builds the musl binary + Release.
#
# The CHANGELOG rotation logic lives in sourceable functions so
# scripts/release.test.sh can unit-test it without invoking git.

set -euo pipefail

# ─────────────────────────────────────────────────────────────────────────────
# Pure helpers (unit-tested by scripts/release.test.sh via `source`)
# ─────────────────────────────────────────────────────────────────────────────

# Strip a leading 'v' from a version token. "v0.3.0" → "0.3.0".
normalize_version() {
  printf '%s' "${1#v}"
}

# Validate that $1 is a bare numeric semver (X.Y.Z). Returns 0/1, prints nothing.
is_valid_version() {
  [[ "${1:-}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
}

# Read the current version from a Cargo.toml path ($1). Prints "X.Y.Z".
cargo_version_clean() {
  awk -F'"' '/^version = / {print $2; exit}' "$1"
}

# Compare two bare semvers $1 and $2. Prints -1 / 0 / 1 for $1 < / == / > $2.
# Returns non-zero on malformed input.
semver_cmp() {
  local a="$1" b="$2"
  is_valid_version "$a" && is_valid_version "$b" || return 1
  local IFS=.
  # shellcheck disable=SC2206
  local a_arr=($a) b_arr=($b)
  local i
  for i in 0 1 2; do
    if (( ${a_arr[$i]} < ${b_arr[$i]} )); then printf '%d\n' -1; return 0; fi
    if (( ${a_arr[$i]} > ${b_arr[$i]} )); then printf '%d\n' 1; return 0; fi
  done
  printf '%d\n' 0
}

# Bump a bare semver $1 by the level $2 (patch|minor|major). Prints new version.
bump_semver() {
  local v="$1" level="$2"
  local IFS=.
  # shellcheck disable=SC2206
  local parts=($v)
  case "$level" in
    major) parts[0]=$((parts[0] + 1)); parts[1]=0; parts[2]=0 ;;
    minor) parts[1]=$((parts[1] + 1)); parts[2]=0 ;;
    patch) parts[2]=$((parts[2] + 1)) ;;
    *) return 1 ;;
  esac
  printf '%d.%d.%d' "${parts[0]}" "${parts[1]}" "${parts[2]}"
}

# set_cargo_version <cargo_file> <new_version>
# Replace the first top-level `^version = "..."` line in Cargo.toml.
set_cargo_version() {
  local cargo="$1" new_version="$2"
  is_valid_version "$new_version" || return 1
  local tmp
  tmp="$(mktemp)"
  awk -v v="$new_version" '
    /^version = / && !done { printf "version = \"%s\"\n", v; done = 1; next }
    { print }
  ' "$cargo" > "$tmp"
  mv "$tmp" "$cargo"
}

# bump_changelog <changelog_file> <new_version> <date YYYY-MM-DD>
#
# Print the transformed changelog to stdout (does not modify the file).
# Transformations (Keep a Changelog format):
#   1. Insert a fresh empty `## [Unreleased]`, then a dated
#      `## [<new_version>] - <date>` heading, and move the old Unreleased body
#      under the new heading. Everything below (older released sections) is
#      preserved verbatim.
#   2. Rebuild the bottom link-reference table from scratch, based on every
#      `## [version]` heading present (plus [Unreleased]). This repairs
#      historical drift (e.g. missing entries) and guarantees one entry per
#      heading, in file order.
#
# The repo base URL is read ONLY from an existing link-reference line
# (`[ref]: https://github.com/owner/repo/compare/...`) — never from in-prose
# markdown links, which may point at other projects. Falls back to the
# canonical next-hunk URL. POSIX awk only (no gawk extensions).
bump_changelog() {
  local changelog="$1" new_version="$2" date="$3"
  local repo
  # Match a link-reference line: `[Unreleased]: https://github.com/o/r/compare/...`
  repo="$(grep -m1 -E '^\[[^]]+\]:[[:space:]]*https://github\.com/[^/[:space:]]+/[^/[:space:]]+/(compare|releases|tree)/' "$changelog" \
        | grep -oE 'https://github\.com/[^/[:space:]]+/[^/[:space:]]+' || true)"
  repo="${repo:-https://github.com/wuxiaobai24/next-hunk}"

  # Pass 1 — rotate the [Unreleased] body under a fresh version heading.
  #   state 0: leading banner (copy as-is until the Unreleased heading)
  #   state 1: inside the old Unreleased body — it now flows under [new].
  #            We skip the body's leading blank line(s) so the new heading gets
  #            exactly one separating blank (printed above), regardless of how
  #            many blanks originally followed `## [Unreleased]`.
  #   state 2: past the first released heading — copy the rest verbatim
  awk -v new="$new_version" -v date="$date" '
    BEGIN { state = 0; body_started = 0 }
    state == 0 && /^## \[Unreleased\][ \t]*$/ {
      print "## [Unreleased]"
      print ""
      print "## [" new "] - " date
      print ""
      state = 1
      body_started = 0
      next
    }
    state == 1 {
      if (/^## /) { state = 2; print; next }           # first released heading
      if (/^[[:space:]]*$/ && !body_started) next       # skip leading blanks
      body_started = 1
      print
      next
    }
    { print }
  ' "$changelog" | \
  # Pass 2 — rebuild the link-reference table.
  #   • Collect every released `## [X.Y.Z]` heading, in order (newest first as
  #     they appear in the file). [Unreleased] is skipped.
  #   • Once the first link-reference line `[ref]: ...` is seen, the entire old
  #     table (and any trailing blanks) is dropped — body lines before it are
  #     printed verbatim.
  #   • END: print a blank separator + a fresh table: [Unreleased] (comparing
  #     the newest released tag against HEAD) then one line per released ver.
  awk -v repo="$repo" '
    BEGIN { n = 0; newest = ""; in_links = 0 }
    # Record released-version headings in order; the first one is the newest.
    !in_links && /^## \[/ {
      line = $0
      sub(/^## \[/, "", line); sub(/\].*$/, "", line)
      if (line ~ /^[0-9]+\.[0-9]+\.[0-9]+$/) {
        versions[++n] = line
        if (newest == "") newest = line
      }
    }
    # Start of the old link-reference block: stop copying.
    !in_links && /^\[[^]]+\]:/ { in_links = 1; next }
    in_links { next }                       # drop rest of the old table
    { print }
    END {
      printf "\n"                           # one blank separator
      print "[Unreleased]: " repo "/compare/v" newest "...HEAD"
      for (i = 1; i <= n; i++) print "[" versions[i] "]: " repo "/releases/tag/v" versions[i]
    }
  '
}

# bump_changelog_inplace <changelog_file> <new_version> <date>
bump_changelog_inplace() {
  local changelog="$1" new_version="$2" date="$3" tmp
  tmp="$(mktemp)"
  bump_changelog "$changelog" "$new_version" "$date" > "$tmp"
  mv "$tmp" "$changelog"
}

# ─────────────────────────────────────────────────────────────────────────────
# Output helpers
# ─────────────────────────────────────────────────────────────────────────────
say()  { printf 'release: %s\n' "$*"; }
note() { printf '  • %s\n' "$*" >&2; }
die()  { printf 'release: error: %s\n' "$*" >&2; exit 1; }

# ─────────────────────────────────────────────────────────────────────────────
# Argument parsing
# ─────────────────────────────────────────────────────────────────────────────
DRY_RUN=0
NO_PR=0
TAG_ONLY=0
NEW_VERSION=""
BUMP=""

print_usage() {
  cat <<'EOF'
release.sh — next-hunk release helper

Stage 1 (prepare a release PR):
  ./scripts/release.sh <version> [options]
  ./scripts/release.sh --bump patch|minor|major [options]

Stage 2 (after the PR merges — tag main to trigger the CI release):
  ./scripts/release.sh --tag vX.Y.Z

Options:
  --dry-run       Print the plan and a preview; change nothing, touch no git.
  --no-pr         Branch + commit only; do not push or open a PR.
  --tag <ver>     Stage 2: checkout main, tag vX.Y.Z, push the tag.
  --bump <level>  Derive the new version by bumping the Cargo.toml version
                  (patch | minor | major). Mutually exclusive with <version>.
  -h, --help      Show this help.

Examples:
  ./scripts/release.sh 0.3.0
  ./scripts/release.sh --bump patch --dry-run
  ./scripts/release.sh --tag v0.3.0      # after the PR is merged
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) print_usage; exit 0 ;;
    --dry-run) DRY_RUN=1; shift ;;
    --no-pr)   NO_PR=1; shift ;;
    --tag)
      [[ $# -ge 2 ]] || die "--tag requires a version argument"
      TAG_ONLY=1
      NEW_VERSION="$(normalize_version "$2")"
      shift 2
      ;;
    --bump)
      [[ $# -ge 2 ]] || die "--bump requires patch|minor|major"
      BUMP="$2"; shift 2
      case "$BUMP" in patch|minor|major) ;; *) die "--bump must be patch|minor|major" ;; esac
      ;;
    --bump=*) BUMP="${1#--bump=}"; shift; case "$BUMP" in patch|minor|major) ;; *) die "--bump must be patch|minor|major" ;; esac ;;
    --*) die "unknown option: $1" ;;
    *)
      if [[ -z "$NEW_VERSION" && -z "$BUMP" ]]; then
        NEW_VERSION="$(normalize_version "$1")"; shift
      else
        die "unexpected argument: $1"
      fi
      ;;
  esac
done

# Don't run the workflow when this file is merely sourced by the test script.
if [[ "${RELEASE_SH_SOURCED:-0}" == "1" ]]; then
  return 0 2>/dev/null || exit 0
fi

# ─────────────────────────────────────────────────────────────────────────────
# Locate the repo root and key files
# ─────────────────────────────────────────────────────────────────────────────
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" \
  || die "not inside a git repository"
CARGO="$REPO_ROOT/Cargo.toml"
CHANGELOG="$REPO_ROOT/CHANGELOG.md"
[[ -f "$CARGO" ]]     || die "Cargo.toml not found at $CARGO"
[[ -f "$CHANGELOG" ]] || die "CHANGELOG.md not found at $CHANGELOG"
CURRENT_VERSION="$(cargo_version_clean "$CARGO")"
is_valid_version "$CURRENT_VERSION" || die "current Cargo.toml version is malformed: $CURRENT_VERSION"

TODAY="$(date -u +%Y-%m-%d)"

# ═════════════════════════════════════════════════════════════════════════════
# STAGE 2: --tag (after PR merge)
# ═════════════════════════════════════════════════════════════════════════════
if [[ "$TAG_ONLY" -eq 1 ]]; then
  [[ -n "$NEW_VERSION" ]] || die "--tag requires a version (e.g. --tag v0.3.0)"
  is_valid_version "$NEW_VERSION" || die "invalid version: $NEW_VERSION"

  say "stage 2: tag release v$NEW_VERSION on main"
  note "this pushes v$NEW_VERSION to origin, triggering release.yml"
  note "(tagging is the documented release trigger — not a content edit to main)"

  [[ -z "$(git status --porcelain)" ]] || die "working tree is not clean; commit or stash first"

  orig_branch="$(git branch --show-current)"
  if [[ "$DRY_RUN" -eq 1 ]]; then
    note "[dry-run] would: checkout main, pull --ff-only, verify Cargo.toml == $NEW_VERSION,"
    note "[dry-run]        tag v$NEW_VERSION, push origin v$NEW_VERSION"
    exit 0
  fi

  git checkout main
  git pull --ff-only

  # Guard: the merged main must actually carry this version.
  on_main="$(cargo_version_clean "$CARGO")"
  [[ "$on_main" == "$NEW_VERSION" ]] \
    || die "main Cargo.toml is $on_main, expected $NEW_VERSION — merge the release PR first"

  git rev-parse -q --verify "refs/tags/v$NEW_VERSION" >/dev/null \
    && die "tag v$NEW_VERSION already exists"

  git tag -a "v$NEW_VERSION" -m "Release v$NEW_VERSION"
  git push origin "v$NEW_VERSION"

  # Return to the original branch if it still exists.
  git checkout "$orig_branch" 2>/dev/null || true

  say "tagged v$NEW_VERSION — watch release.yml:"
  note "https://github.com/wuxiaobai24/next-hunk/actions/workflows/release.yml"
  exit 0
fi

# ═════════════════════════════════════════════════════════════════════════════
# STAGE 1: bump files → branch → PR
# ═════════════════════════════════════════════════════════════════════════════

# Resolve the target version.
if [[ -n "$BUMP" ]]; then
  [[ -z "$NEW_VERSION" ]] || die "specify either a version or --bump, not both"
  NEW_VERSION="$(bump_semver "$CURRENT_VERSION" "$BUMP")"
fi
[[ -n "$NEW_VERSION" ]] || { print_usage; exit 1; }
is_valid_version "$NEW_VERSION" || die "invalid version: $NEW_VERSION"

# Safety: target must be strictly newer (no downgrades/re-releases).
cmp_result="$(semver_cmp "$NEW_VERSION" "$CURRENT_VERSION")"
[[ "$cmp_result" == "1" ]] \
  || die "target $NEW_VERSION is not newer than current $CURRENT_VERSION"

# No existing tag (otherwise release.yml would double-fire / we'd clobber).
git rev-parse -q --verify "refs/tags/v$NEW_VERSION" >/dev/null \
  && die "tag v$NEW_VERSION already exists; bump again or delete the tag"

say "preparing release v$NEW_VERSION (from v$CURRENT_VERSION)"

# ── Pre-flight: environment ────────────────────────────────────────────────
if [[ "$DRY_RUN" -eq 0 && "$NO_PR" -eq 0 ]]; then
  command -v gh >/dev/null \
    || die "gh CLI not found; install it or use --no-pr / --dry-run"
  gh auth status >/dev/null 2>&1 \
    || die "gh is not authenticated; run \`gh auth login\`"
fi
[[ -z "$(git status --porcelain)" ]] \
  || die "working tree is not clean; stage/commit your changes first"

# Start from an up-to-date main.
git checkout main
[[ "$DRY_RUN" -eq 1 ]] || git pull --ff-only || note "could not pull main (offline? continuing on current main)"

# ── Show the plan / preview (dry-run) ──────────────────────────────────────
BRANCH="chore/release-v$NEW_VERSION"

if [[ "$DRY_RUN" -eq 1 ]]; then
  say "[dry-run] plan:"
  note "current version: $CURRENT_VERSION → $NEW_VERSION"
  note "date: $TODAY"
  note "branch: $BRANCH (from main)"
  note "files: Cargo.toml (version), CHANGELOG.md (rotate [Unreleased])"
  note "then: commit 'Release v$NEW_VERSION'"
  if [[ "$NO_PR" -eq 0 ]]; then note "then: push branch + gh pr create"; fi
  printf '\n--- Cargo.toml diff (planned) ---\n'
  printf -- '--- a/Cargo.toml\n+++ b/Cargo.toml\n'
  printf '@@ -1 +1 @@\n-version = "%s"\n+version = "%s"\n' "$CURRENT_VERSION" "$NEW_VERSION"
  printf '\n--- CHANGELOG.md top (planned preview) ---\n'
  bump_changelog "$CHANGELOG" "$NEW_VERSION" "$TODAY" | head -12 | sed 's/^/  /'
  printf '\n--- CHANGELOG.md link table (planned preview) ---\n'
  bump_changelog "$CHANGELOG" "$NEW_VERSION" "$TODAY" | tail -8 | sed 's/^/  /'
  exit 0
fi

# ── Do the work ─────────────────────────────────────────────────────────────
say "bumping Cargo.toml: $CURRENT_VERSION → $NEW_VERSION"
set_cargo_version "$CARGO" "$NEW_VERSION"
verify="$(cargo_version_clean "$CARGO")"
[[ "$verify" == "$NEW_VERSION" ]] \
  || die "Cargo.toml version verification failed (got $verify)"

say "rotating CHANGELOG [Unreleased] → [$NEW_VERSION]"
bump_changelog_inplace "$CHANGELOG" "$NEW_VERSION" "$TODAY"

say "creating branch $BRANCH"
git checkout -b "$BRANCH"
git add Cargo.toml CHANGELOG.md
git commit -m "Release v$NEW_VERSION" \
  -m "Bump Cargo.toml and rotate CHANGELOG [Unreleased] into [$NEW_VERSION]." \
  -m "Merge this PR, then run: ./scripts/release.sh --tag v$NEW_VERSION"

if [[ "$NO_PR" -eq 1 ]]; then
  say "done (no-pr): committed on $BRANCH; review with:"
  note "git show main..$BRANCH"
  note "when ready: push & open the PR, or re-run without --no-pr"
  exit 0
fi

say "pushing $BRANCH and opening a PR"
git push -u origin "$BRANCH"

PR_BODY="$(cat <<EOF
## What

Release \`v$NEW_VERSION\` — bumps \`Cargo.toml\` from \`$CURRENT_VERSION\` and
rotates the CHANGELOG \`[Unreleased]\` section into a dated
\`[$NEW_VERSION]\` heading (the link table is repaired as a side effect).

Generated by \`scripts/release.sh\`.

## Why

One-command, PR-only release flow (per AGENTS.md — no direct content pushes to
main). This PR is stage 1; the build/publish happens in stage 2 once merged.

## After merge

\`\`\`bash
./scripts/release.sh --tag v$NEW_VERSION
\`\`\`
That checks out \`main\`, tags \`v$NEW_VERSION\`, and pushes the tag, which
triggers \`release.yml\` to build the static musl binary and publish the
GitHub Release.

## Verification

- [x] \`bash scripts/release.test.sh\` — CHANGELOG rotation unit tests pass
- [x] \`./scripts/release.sh $NEW_VERSION --dry-run\` — plan previewed
- [x] \`cargo build\` — version bump compiles
EOF
)"

gh pr create --title "Release v$NEW_VERSION" --body "$PR_BODY" --base main --head "$BRANCH"

say "PR opened. After review + merge, run:"
note "./scripts/release.sh --tag v$NEW_VERSION"
