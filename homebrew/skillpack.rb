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
# TODO: re-pin sha256 via scripts/update_homebrew_sha256.py after the release is published
    else
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.2/skillpack-x86_64-apple-darwin.tar.gz"
# TODO: re-pin sha256 via scripts/update_homebrew_sha256.py after the release is published
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.2/skillpack-aarch64-unknown-linux-gnu.tar.gz"
# TODO: re-pin sha256 via scripts/update_homebrew_sha256.py after the release is published
    else
      # Static musl build - no glibc dependency.
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.2/skillpack-x86_64-unknown-linux-musl.tar.gz"
# TODO: re-pin sha256 via scripts/update_homebrew_sha256.py after the release is published
    end
  end

  def install
    bin.install "skillpack"
  end

  test do
    system "#{bin}/skillpack", "--version"
  end
end
