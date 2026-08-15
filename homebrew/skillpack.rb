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
# the `url`s below. Add a `sha256` per platform once you have pinned a release
# (Homebrew warns without it but still installs):
#   curl -fsSL <url> | shasum -a 256
class Skillpack < Formula
  desc "Generate and verify the agent-distribution layer for any OSS project"
  homepage "https://github.com/nordicnode/skillpack"
  license "MIT"
  version "0.13.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.0/skillpack-aarch64-apple-darwin.tar.gz"
    else
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.0/skillpack-x86_64-apple-darwin.tar.gz"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.0/skillpack-aarch64-unknown-linux-gnu.tar.gz"
    else
      # Static musl build — no glibc dependency.
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.0/skillpack-x86_64-unknown-linux-musl.tar.gz"
    end
  end

  def install
    bin.install "skillpack"
  end

  test do
    system "#{bin}/skillpack", "--version"
  end
end
