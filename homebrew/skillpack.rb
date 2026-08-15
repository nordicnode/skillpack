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
  version "0.13.1"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.1/skillpack-aarch64-apple-darwin.tar.gz"
      sha256 "ff6840259d12744ee240fe88dd44b0ce6d85ce1016077632de82917334652adb"
    else
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.1/skillpack-x86_64-apple-darwin.tar.gz"
      sha256 "cc79fbf364a7a4553b752ce935bf8d6cfde1126c70865065f7bb82de337d83d5"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.1/skillpack-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "19d21d4bfdb36ad725771892e37628c854039fa39753d0a8477def872dd6b30e"
    else
      # Static musl build - no glibc dependency.
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.1/skillpack-x86_64-unknown-linux-musl.tar.gz"
      sha256 "3213f26ddc527f07c37ff5c393a5d7aa19fa456bf900427fe5cb86e3d6e4f75a"
    end
  end

  def install
    bin.install "skillpack"
  end

  test do
    system "#{bin}/skillpack", "--version"
  end
end
