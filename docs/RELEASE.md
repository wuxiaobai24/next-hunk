# Releasing next-hunk

Cut a release only from `main` after CI is green. Never publish without a
git tag — the release workflow is triggered solely by `push` of `v*` tags
(see `.github/workflows/release.yml`).

## Preconditions

1. `Cargo.toml` `version` matches the intended tag (e.g. `0.4.0` → `v0.4.0`).
2. `CHANGELOG.md` has a dated section for that version; footer compare links
   point at the new tag.
3. Repository secret **`CARGO_REGISTRY_TOKEN`** is set (crates.io API token
   with publish scope). Without it the GitHub Release still publishes
   binaries, but the crates.io job skips with a notice.
4. You are not force-pushing tags; re-publishing the same crate version fails
   by design on crates.io.

## Cut the tag (human gate)

```bash
git checkout main
git pull --ff-only
# confirm version
rg '^version' Cargo.toml
# annotated tag from the release commit on main
git tag -a v0.4.0 -m "next-hunk 0.4.0"
git push origin v0.4.0
```

Watch the `release` workflow: four matrix builds → `github-release` assets
(`.tar.xz` + `.sha256`) → optional `crates.io publish`.

## After the tag is green

### Verify install paths

At least two of three should yield the same major version:

```bash
# 1) prebuilt installer
curl -fsSL https://github.com/wuxiaobai24/next-hunk/raw/main/scripts/install.sh \
  | bash -s -- --version 0.4.0 --bin-dir /tmp/nh-bin --force
/tmp/nh-bin/next-hunk --version   # expect: next-hunk 0.4.0

# 2) crates.io (after publish job succeeds)
cargo install next-hunk --version 0.4.0 --locked --force
next-hunk --version

# 3) Homebrew (macOS / Linuxbrew) — update formula sha256 first (below)
brew install --formula \
  https://raw.githubusercontent.com/wuxiaobai24/next-hunk/main/Formula/next-hunk.rb
next-hunk --version
```

Release page must list four platform archives and matching `.sha256` files:
`x86_64-musl`, `aarch64-musl`, `aarch64-apple-darwin`, `x86_64-apple-darwin`.

### Bump Homebrew formula sha256

GitHub only materializes the source tarball after the tag exists:

```bash
SHA=$(curl -fsSL \
  "https://github.com/wuxiaobai24/next-hunk/archive/refs/tags/v0.4.0.tar.gz" \
  | sha256sum | awk '{print $1}')
echo "$SHA"
# edit Formula/next-hunk.rb: url → v0.4.0, sha256 → $SHA
# open a small follow-up PR if the release PR could not know the digest yet
```

## What this workflow does not do

- Does **not** run on branch pushes or PRs (no accidental crates.io publish).
- Does **not** create the tag for you — tagging is the human/ops gate.
- Does **not** update the Homebrew formula digest automatically.
