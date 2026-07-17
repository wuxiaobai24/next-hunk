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
class NextHunk < Formula
  desc "High-performance terminal review engine for large changesets"
  homepage "https://github.com/wuxiaobai24/next-hunk"
  url "https://github.com/wuxiaobai24/next-hunk/archive/refs/tags/v0.3.0.tar.gz"
  sha256 "adf3c2ccb037b3a832ef9ab36a7efe0f9eda8d131bc72130bec48d123292be61"
  license "MIT"
  head "https://github.com/wuxiaobai24/next-hunk.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/next-hunk --version")
  end
end
