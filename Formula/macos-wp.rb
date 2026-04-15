# Update `version` and the two sha256 values after each tagged release.
# The sha256 values are printed as `.tar.gz.sha256` files attached to each
# GitHub release by the `release` workflow.

class MacosWp < Formula
  desc "CLI for managing macOS wallpapers per display (survives new Spaces)"
  homepage "https://github.com/marek-vybiral/homebrew-macos-wp"
  version "0.1.0"
  license "Unlicense"

  on_macos do
    on_arm do
      url "https://github.com/marek-vybiral/homebrew-macos-wp/releases/download/v#{version}/macos-wp-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "fd2df22120b1b96614532fe357472709c77067c0a9e8d2381f4433406b94de77"
    end
    on_intel do
      url "https://github.com/marek-vybiral/homebrew-macos-wp/releases/download/v#{version}/macos-wp-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "99a4094280fcc4c1fc1b16648101da49639c6bc2078049775895e3e33be263f3"
    end
  end

  def install
    bin.install "macos-wp"
  end

  test do
    assert_match "macos-wp", shell_output("#{bin}/macos-wp --version")
  end
end
