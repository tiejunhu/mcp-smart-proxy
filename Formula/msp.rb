class Msp < Formula
  desc "Smart proxy for multiple stdio MCP servers"
  homepage "https://github.com/tiejunhu/mcp-smart-proxy"
  version "0.0.2"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/tiejunhu/mcp-smart-proxy/releases/download/v0.0.2/mcp-smart-proxy-v0.0.2-aarch64-apple-darwin.tar.gz"
      sha256 "1443d529b1e8329b1a4db342cb4c0fe474415d4e1131e9c4a3cbb7cdf7b2e9db"
    else
      url "https://github.com/tiejunhu/mcp-smart-proxy/releases/download/v0.0.2/mcp-smart-proxy-v0.0.2-x86_64-apple-darwin.tar.gz"
      sha256 "fd374047de27029f929188bb7edb152d9475220b53b616e5d15bbddb7706a5f8"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/tiejunhu/mcp-smart-proxy/releases/download/v0.0.2/mcp-smart-proxy-v0.0.2-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "5b7b819a1afcb0eaf9dbf2e1aea47185d30371f6deff2be6cf17c1a0a45bb47b"
    else
      url "https://github.com/tiejunhu/mcp-smart-proxy/releases/download/v0.0.2/mcp-smart-proxy-v0.0.2-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "a0b277f5efbaec7ad4f25df4d6f35b8df0ca8d851d73c0abe4bccb1881289e69"
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
