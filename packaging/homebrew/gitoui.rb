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
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.6/gitoui-0.3.6-aarch64-apple-darwin.tar.gz"
      sha256 "cd56fb906c866ff50bfa7f3d504cf308a0f78740ab1db635720f1914130aab89"
    end
    on_intel do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.6/gitoui-0.3.6-x86_64-apple-darwin.tar.gz"
      sha256 "1be43dfb882fd5436f3f3ef0368af9b272e28ec3c1f4592b4a14ba999bc6de68"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.6/gitoui-0.3.6-aarch64-unknown-linux-musl.tar.gz"
      sha256 "9234d1daaeb2a70767a223adaf49ea9804b39c1f521f33fdbfcaba744fa5280d"
    end
    on_intel do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.3.6/gitoui-0.3.6-x86_64-unknown-linux-musl.tar.gz"
      sha256 "b9fe6ab148ff09180245a5f8061887d6d2f13015e538c8ff80cde04912830a45"
    end
  end

  def install
    bin.install "gitoui"
  end

  test do
    assert_match "gitoui #{version}", shell_output("#{bin}/gitoui --version")
  end
end
