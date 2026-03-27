#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 7 ]]; then
  echo "usage: $0 <output-path> <version> <asset-prefix> <macos-arm64-sha256> <macos-x86_64-sha256> <linux-arm64-sha256> <linux-x86_64-sha256>" >&2
  exit 1
fi

output_path="$1"
version="$2"
asset_prefix="$3"
macos_arm64_sha256="$4"
macos_x86_64_sha256="$5"
linux_arm64_sha256="$6"
linux_x86_64_sha256="$7"

mkdir -p "$(dirname "${output_path}")"

cat > "${output_path}" <<EOF
class Msp < Formula
  desc "Smart proxy for multiple stdio MCP servers"
  homepage "https://github.com/cybershape/mcp-smart-proxy"
  version "${version}"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/cybershape/mcp-smart-proxy/releases/download/v${version}/${asset_prefix}-v${version}-aarch64-apple-darwin.tar.gz"
      sha256 "${macos_arm64_sha256}"
    else
      url "https://github.com/cybershape/mcp-smart-proxy/releases/download/v${version}/${asset_prefix}-v${version}-x86_64-apple-darwin.tar.gz"
      sha256 "${macos_x86_64_sha256}"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/cybershape/mcp-smart-proxy/releases/download/v${version}/${asset_prefix}-v${version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "${linux_arm64_sha256}"
    else
      url "https://github.com/cybershape/mcp-smart-proxy/releases/download/v${version}/${asset_prefix}-v${version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "${linux_x86_64_sha256}"
    end
  end

  def install
    binary = Dir["msp", "*/msp", "mcp-smart-proxy", "*/mcp-smart-proxy"].first
    raise "msp binary not found in archive" unless binary

    bin.install binary => "msp"

    metafiles_dir = Dir["README.md", "LICENSE*", "COPYING*", "*/README.md", "*/LICENSE*", "*/COPYING*"].empty? ? nil : Dir["*", "."].find do |entry|
      next false unless File.directory?(entry)

      Dir[File.join(entry, "README.md"), File.join(entry, "LICENSE*"), File.join(entry, "COPYING*")].any?
    end

    prefix.install_metafiles(metafiles_dir || ".")
  end

  test do
    assert_match "A smart MCP proxy", shell_output("#{bin}/msp --help")
  end
end
EOF
