#!/usr/bin/env sh
# skillpack one-command installer.
#
# Downloads the prebuilt binary for the current platform from the GitHub
# Release for a given version and installs it to ~/.local/bin (or
# $SKILLPACK_INSTALL_DIR). No Rust toolchain required.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/nordicnode/skillpack/main/install.sh | sh
#   # or pin a version:
#   VERSION=0.13.0 curl -fsSL .../install.sh | sh
#
# Env vars:
#   VERSION               release tag to install (defaults to the latest release)
#   SKILLPACK_INSTALL_DIR destination directory (default: ~/.local/bin)

set -eu

repo="nordicnode/skillpack"
bin="skillpack"
version="${VERSION:-}"

# --- resolve the version -----------------------------------------------------
if [ -z "$version" ]; then
  version="$(curl -fsSL "https://api.github.com/repos/${repo}/releases/latest" \
    | grep '"tag_name"' | head -n1 | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
fi
case "$version" in
  v*) : ;;
  *) version="v${version}" ;;
esac

# --- resolve the platform triple -------------------------------------------
os="$(uname -s)"
arch="$(uname -m)"
case "${os}-${arch}" in
  Linux-x86_64)    target="x86_64-unknown-linux-musl" ;;
  Linux-aarch64|Linux-arm64) target="aarch64-unknown-linux-musl" ;;
  Darwin-x86_64)   target="x86_64-apple-darwin" ;;
  Darwin-arm64)    target="aarch64-apple-darwin" ;;
  *) echo "error: unsupported platform ${os}-${arch} (try cargo install skillpack)" >&2; exit 1 ;;
esac

install_dir="${SKILLPACK_INSTALL_DIR:-$HOME/.local/bin}"
mkdir -p "$install_dir"

url="https://github.com/${repo}/releases/download/${version}/${bin}-${target}.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "installing skillpack ${version} (${target}) to ${install_dir}"
curl -fsSL "$url" | tar -xz -C "$tmp"
install -m 0755 "$tmp/${bin}" "$install_dir/${bin}"

# Confirm it runs; surface a PATH hint when the dir isn't already on PATH.
"$install_dir/${bin}" --version
case ":$PATH:" in
  *":${install_dir}:"*) ;;
  *) echo "note: add ${install_dir} to your PATH" >&2 ;;
esac

echo "done: ${install_dir}/${bin}"
