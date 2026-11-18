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
      url "https://github.com/NayJi7/gitoui/releases/download/v0.4.0/gitoui-0.4.0-aarch64-apple-darwin.tar.gz"
      sha256 "a6b03737c993c39bc39b982a5c31fc0f9baefb26eab2fda2ac9961874702c0f3"
    end
    on_intel do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.4.0/gitoui-0.4.0-x86_64-apple-darwin.tar.gz"
      sha256 "cbedca96294f72e0286919cc9556ca4f3da52ba8e5107d79d8617c28903dd897"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.4.0/gitoui-0.4.0-aarch64-unknown-linux-musl.tar.gz"
      sha256 "d98db13545df089eb15fa787d603c5f4afb83ba8beb6dbcec5eebdce27d4fb9b"
    end
    on_intel do
      url "https://github.com/NayJi7/gitoui/releases/download/v0.4.0/gitoui-0.4.0-x86_64-unknown-linux-musl.tar.gz"
      sha256 "8318dd9d1cfed9c3364790172a976860df3b9d359106c1c13038254164627a49"
    end
  end

  def install
    bin.install "gitoui"
  end

  test do
    assert_match "gitoui #{version}", shell_output("#{bin}/gitoui --version")
  end
end
