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
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.5/gitoui-0.3.5-aarch64-apple-darwin.tar.gz"
      sha256 "3bf18e5f7884be66b16cbaf78242864738ed99beacfb3dd0770b933b70982b5e"
    end
    on_intel do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.5/gitoui-0.3.5-x86_64-apple-darwin.tar.gz"
      sha256 "281620f45e7eb506b2c9845ba35dd1cf34b6eaf5f073b3e4de27063d1236a198"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.5/gitoui-0.3.5-aarch64-unknown-linux-musl.tar.gz"
      sha256 "1db1e0a835b05cad06e3902ded801c9fa61d81d016ef5acd215f776b9b69db12"
    end
    on_intel do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.5/gitoui-0.3.5-x86_64-unknown-linux-musl.tar.gz"
      sha256 "2444b5bdb24eb32ab694f400797f22aa13cdb238b8b60a3d07ce02aa997d63ac"
    end
  end

  def install
    bin.install "gitoui"
  end

  test do
    assert_match "gitoui #{version}", shell_output("#{bin}/gitoui --version")
  end
end
