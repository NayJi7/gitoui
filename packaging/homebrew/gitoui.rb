# Homebrew formula for gitoui, served from the NayJi7/homebrew-tap repository:
#
#   brew install nayji7/tap/gitoui
#
# Installs the prebuilt release binaries (Linux uses the static musl builds).
# On every release, bump the version in the four URLs and the four sha256
# values from the release's checksum.txt.
class Gitoui < Formula
  desc "Terminal Git client with an image-rendered commit graph and GitHub PRs"
  homepage "https://nayji7.github.io/gitoui/"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.3/gitoui-0.3.3-aarch64-apple-darwin.tar.gz"
      sha256 "767c1faf69c12038537445adda8a88b5006713ab22f6dd41fb178f69239d6d1d"
    end
    on_intel do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.3/gitoui-0.3.3-x86_64-apple-darwin.tar.gz"
      sha256 "9808f357d3f7001af2d9c1df0512a1a6df1ec7076cead6f1d9a439be367b6d93"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.3/gitoui-0.3.3-aarch64-unknown-linux-musl.tar.gz"
      sha256 "be50c22be529f6b21d4a52b482020ecc85dc13e112e654d47f710bb367bbd75c"
    end
    on_intel do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.3/gitoui-0.3.3-x86_64-unknown-linux-musl.tar.gz"
      sha256 "70b35a63d812f352fb07e3a75715413f88487b03b87b4232c79765dc509ad810"
    end
  end

  def install
    bin.install "gitoui"
  end

  test do
    assert_match "gitoui #{version}", shell_output("#{bin}/gitoui --version")
  end
end
