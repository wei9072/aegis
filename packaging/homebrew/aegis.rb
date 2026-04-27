# V2.0 — Homebrew formula template.
#
# Copy into a tap repo at e.g. `homebrew-aegis/Formula/aegis.rb`,
# then `brew tap wei9072/aegis && brew install aegis`.
#
# The SHA256 placeholders below are filled in per release by:
#
#   curl -L https://github.com/wei9072/aegis/releases/download/<tag>/<asset>.tar.gz \
#     | shasum -a 256
#
# Or — more sustainably — automate via cargo-dist's Homebrew
# generator (https://opensource.axo.dev/cargo-dist/book/installers/homebrew.html).

class Aegis < Formula
  desc "Behavior harness for LLM-driven workflows. Rejects regressions instead of teaching the model."
  homepage "https://github.com/wei9072/aegis"
  version "0.1.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/wei9072/aegis/releases/download/v#{version}/aegis-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_AARCH64_DARWIN_SHA256"
    end
    on_intel do
      url "https://github.com/wei9072/aegis/releases/download/v#{version}/aegis-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_X86_64_DARWIN_SHA256"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/wei9072/aegis/releases/download/v#{version}/aegis-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_AARCH64_LINUX_SHA256"
    end
    on_intel do
      url "https://github.com/wei9072/aegis/releases/download/v#{version}/aegis-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_X86_64_LINUX_SHA256"
    end
  end

  def install
    bin.install "aegis"
    bin.install "aegis-mcp"
  end

  test do
    assert_match "python", shell_output("#{bin}/aegis languages")
    # Smoke-test the MCP server's handshake.
    output = pipe_output(
      "#{bin}/aegis-mcp",
      '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'
    )
    assert_match "protocolVersion", output
  end
end
