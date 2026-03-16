#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 6 ]]; then
  echo "usage: $0 <version> <asset-prefix> <macos-arm64-sha256> <macos-x86_64-sha256> <linux-arm64-sha256> <linux-x86_64-sha256>" >&2
  exit 1
fi

version="$1"
asset_prefix="$2"
macos_arm64_sha256="$3"
macos_x86_64_sha256="$4"
linux_arm64_sha256="$5"
linux_x86_64_sha256="$6"

mkdir -p Formula

cat > Formula/msp.rb <<EOF
class Msp < Formula
  desc "Smart proxy for multiple stdio MCP servers"
  homepage "https://github.com/tiejunhu/mcp-smart-proxy"
  version "${version}"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/tiejunhu/mcp-smart-proxy/releases/download/v${version}/${asset_prefix}-v${version}-aarch64-apple-darwin.tar.gz"
      sha256 "${macos_arm64_sha256}"
    else
      url "https://github.com/tiejunhu/mcp-smart-proxy/releases/download/v${version}/${asset_prefix}-v${version}-x86_64-apple-darwin.tar.gz"
      sha256 "${macos_x86_64_sha256}"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/tiejunhu/mcp-smart-proxy/releases/download/v${version}/${asset_prefix}-v${version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "${linux_arm64_sha256}"
    else
      url "https://github.com/tiejunhu/mcp-smart-proxy/releases/download/v${version}/${asset_prefix}-v${version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "${linux_x86_64_sha256}"
    end
  end

  def install
    binary = Dir["msp", "*/msp", "mcp-smart-proxy", "*/mcp-smart-proxy"].first
    raise "msp binary not found in archive" unless binary

    readme = Dir["README.md", "*/README.md"].first
    raise "README.md not found in archive" unless readme

    bin.install binary => "msp"
    prefix.install_metafiles readme
  end

  test do
    assert_match "A smart MCP proxy", shell_output("#{bin}/msp --help")
  end
end
EOF
