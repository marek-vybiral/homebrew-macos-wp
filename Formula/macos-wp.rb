# Update `version` and the two sha256 values after each tagged release.
# The sha256 values are printed as `.tar.gz.sha256` files attached to each
# GitHub release by the `release` workflow.

class MacosWp < Formula
  desc "CLI for managing macOS wallpapers per display (survives new Spaces)"
  homepage "https://github.com/YOUR_GH_USERNAME/homebrew-macos-wp"
  version "0.1.0"
  license "Unlicense"

  on_macos do
    on_arm do
      url "https://github.com/YOUR_GH_USERNAME/homebrew-macos-wp/releases/download/v#{version}/macos-wp-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_AARCH64_SHA256"
    end
    on_intel do
      url "https://github.com/YOUR_GH_USERNAME/homebrew-macos-wp/releases/download/v#{version}/macos-wp-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_X86_64_SHA256"
    end
  end

  def install
    bin.install "macos-wp"
  end

  test do
    assert_match "macos-wp", shell_output("#{bin}/macos-wp --version")
  end
end
