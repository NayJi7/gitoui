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
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.2/gitoui-0.3.2-aarch64-apple-darwin.tar.gz"
      sha256 "87efce7129cba8113cf7796a661dc200d9c12e6de1224236021d46f134932b57"
    end
    on_intel do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.2/gitoui-0.3.2-x86_64-apple-darwin.tar.gz"
      sha256 "caf2bb92b6e9de8bc6d23012ffb5f74cc572e4933402cfd97ffc28ca3dc909cf"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.2/gitoui-0.3.2-aarch64-unknown-linux-musl.tar.gz"
      sha256 "90c1111a5e1d8397d74668491f6628c0ab2ffb0f88788f01e3ad348aa6cb4563"
    end
    on_intel do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.2/gitoui-0.3.2-x86_64-unknown-linux-musl.tar.gz"
      sha256 "5b5bbd0031cb2d16e56cce6e788552d977f7b4f6cd8f8c6100052e47976d2b82"
    end
  end

  def install
    bin.install "gitoui"
  end

  test do
    assert_match "gitoui #{version}", shell_output("#{bin}/gitoui --version")
  end
end
