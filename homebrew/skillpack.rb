# Homebrew formula for skillpack.
#
# Ships the prebuilt binaries attached to the GitHub Release (built by the
# Release workflow). To use it as a tap:
#
#   brew tap nordicnode/skillpack https://github.com/nordicnode/skillpack
#   brew install skillpack
#
# Or install this single file directly:
#
#   brew install --formula homebrew/skillpack.rb
#
# Version/URL bumps: on each release update `version` and the `vX.Y.Z` tag in
# the `url`s below. The `sha256` pins are per-binary and must be regenerated
# after each release (they cannot be computed until the binaries exist):
#
#   python3 scripts/update_homebrew_sha256.py
#
# The release-plz sync step strips the `sha256` lines when it bumps the
# version, so a stale checksum can never ship; re-pin with the script above
# once the release is published.
class Skillpack < Formula
  desc "Generate and verify the agent-distribution layer for any OSS project"
  homepage "https://github.com/nordicnode/skillpack"
  license "MIT"
  version "0.13.2"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.2/skillpack-aarch64-apple-darwin.tar.gz"
      sha256 "c76105f574f27b1043e386934db6a7b90ce612e18212c62b8e593c136890d7b7"
    else
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.2/skillpack-x86_64-apple-darwin.tar.gz"
      sha256 "921a6439ee54b172e6fd62ab9f8efc5f49a81e854451016c538d01ec23a6126b"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.2/skillpack-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "e21e739bc638cedc168a96a3cc46dae63536ab423aaba15ff4bcdcd71e1fd680"
    else
      # Static musl build - no glibc dependency.
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.2/skillpack-x86_64-unknown-linux-musl.tar.gz"
      sha256 "85b1b0ace70d706758ad980716c2e9facc39d679e583532a83cc556548013760"
    end
  end

  def install
    bin.install "skillpack"
  end

  test do
    system "#{bin}/skillpack", "--version"
  end
end
