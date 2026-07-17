# Homebrew formula for next-hunk.
#
# Install (custom tap pointing at this repo):
#   brew tap wuxiaobai24/next-hunk https://github.com/wuxiaobai24/next-hunk
#   brew install next-hunk
#
# Or one-shot without a permanent tap:
#   brew install --formula https://raw.githubusercontent.com/wuxiaobai24/next-hunk/main/Formula/next-hunk.rb
#
# Builds from the tagged source with cargo (Rust is a build dependency).
# Prebuilt multi-platform archives are also published on GitHub Releases and
# installed by scripts/install.sh without needing a Rust toolchain.
#
# After tagging a new version, refresh `url` + `sha256` (see docs/RELEASE.md):
#   curl -fsSL https://github.com/wuxiaobai24/next-hunk/archive/refs/tags/vX.Y.Z.tar.gz \
#     | sha256sum
class NextHunk < Formula
  desc "High-performance terminal review engine for large changesets"
  homepage "https://github.com/wuxiaobai24/next-hunk"
  # Points at the latest *published* tag. Bump with Cargo.toml after tag cut;
  # sha256 is of the GitHub-generated source archive (not the dist binary).
  url "https://github.com/wuxiaobai24/next-hunk/archive/refs/tags/v0.3.0.tar.gz"
  sha256 "adf3c2ccb037b3a832ef9ab36a7efe0f9eda8d131bc72130bec48d123292be61"
  license "MIT"
  head "https://github.com/wuxiaobai24/next-hunk.git", branch: "main"
  # v0.4.0: after `git push origin v0.4.0` and release workflow is green, set:
  #   url "…/tags/v0.4.0.tar.gz"
  #   sha256 "<from docs/RELEASE.md>"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/next-hunk --version")
  end
end
