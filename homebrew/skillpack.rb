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
# the `url`s below. The `sha256` pins are per-binary and cannot be computed
# until the binaries exist, so the Release workflow's `re-pin-homebrew` job
# regenerates them automatically when the `v*` tag is pushed (it runs
# scripts/update_homebrew_sha256.py and commits the result to main). For an
# out-of-band fix, run the script by hand:
#
#   python3 scripts/update_homebrew_sha256.py
#
# The release-plz sync step strips the `sha256` lines when it bumps the
# version, so a stale checksum can never ship.
class Skillpack < Formula
  desc "Generate and verify the agent-distribution layer for any OSS project"
  homepage "https://github.com/nordicnode/skillpack"
  license "MIT"
  version "0.13.3"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.3/skillpack-aarch64-apple-darwin.tar.gz"
      sha256 "be0a4e97df556cf4d4b5b882ac1971bd96c6477cb5b90edd8033380ca14a4361"
    else
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.3/skillpack-x86_64-apple-darwin.tar.gz"
      sha256 "4c8a33fe058727679a6ace5e16a7069deff77023c2b312e91dac44324bc31abe"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.3/skillpack-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "9a1bbeb27bebe6f46c18c4a4baa83fba53d24e83f0c26b5d2b58d3c69948dd11"
    else
      # Static musl build - no glibc dependency.
      url "https://github.com/nordicnode/skillpack/releases/download/v0.13.3/skillpack-x86_64-unknown-linux-musl.tar.gz"
      sha256 "aeccc3dffcfe1d5b5384eefa49384798ee4e1e37b872f940c13bbef5ac2e47e8"
    end
  end

  def install
    bin.install "skillpack"
  end

  test do
    system "#{bin}/skillpack", "--version"
  end
end
