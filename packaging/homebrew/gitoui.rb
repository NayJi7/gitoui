# Homebrew formula for gitoui, served from the NayJi7/homebrew-tap repository:
#
#   brew install nayji7/tap/gitoui
#
# Installs the prebuilt release binaries (Linux uses the static musl builds).
# On every release, bump the version in the four URLs and the four sha256
# values from the release's checksum.txt.
class Gitoui < Formula
  desc "Terminal Git client with an image-rendered commit graph and GitHub PRs"
  homepage "https://gitoui.nayji7.dev/"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.7/gitoui-0.3.7-aarch64-apple-darwin.tar.gz"
      sha256 "2a61de31d0e5f4afc8624752f547d9f609ea1c6b5828e18a8dca0c38a3c86638"
    end
    on_intel do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.7/gitoui-0.3.7-x86_64-apple-darwin.tar.gz"
      sha256 "123ba9c7ea36e349372f9fab66db1c8444f4db8d3b52561f7824bdbd674bfc3e"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.7/gitoui-0.3.7-aarch64-unknown-linux-musl.tar.gz"
      sha256 "4034b3190065e692cca6e0b10dca35da2865b7d70a3d007c339a0dcb70d2bdfc"
    end
    on_intel do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.7/gitoui-0.3.7-x86_64-unknown-linux-musl.tar.gz"
      sha256 "a3c0cf1189d8a2650947d17803c08325e95bb4efc5e086951ff15eaec4a0ddcf"
    end
  end

  def install
    bin.install "gitoui"
  end

  test do
    assert_match "gitoui #{version}", shell_output("#{bin}/gitoui --version")
  end
end
