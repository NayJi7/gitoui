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
      url "https://github.com/NayJi7/gitoui/releases/download/v0.5.0/gitoui-0.5.0-aarch64-apple-darwin.tar.gz"
      sha256 "406ea683140039b04771d8f568aa43b9279d1553e4c42e9d1a6f1b14bd133016"
    end
    on_intel do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.5.0/gitoui-0.5.0-x86_64-apple-darwin.tar.gz"
      sha256 "c4a375d68debfe5135c12e84eb0a33e92d18fa04ea71a0f22903c3d2676d23c3"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.5.0/gitoui-0.5.0-aarch64-unknown-linux-musl.tar.gz"
      sha256 "9edc150957d4ca524cdd0d6f3dacfa690c0e428d549c59819ce7e0f0596a3ac4"
    end
    on_intel do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.5.0/gitoui-0.5.0-x86_64-unknown-linux-musl.tar.gz"
      sha256 "ca0cfd634bb3f72e1b8e2b4d28f85fd9c217a3d292ea2dbd066bd844e121b017"
    end
  end

  def install
    bin.install "gitoui"
  end

  test do
    assert_match "gitoui #{version}", shell_output("#{bin}/gitoui --version")
  end
end
