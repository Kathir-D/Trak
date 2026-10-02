# MIT License — Copyright (c) 2026 Trak contributors (see LICENSE). Trak descends
# from shpotify by Harish Narayanan; see THIRD-PARTY-NOTICES.md.
#
# Homebrew formula for Trak. It ships the prebuilt universal binary from the GitHub
# release tarball, which scripts/package-release.sh builds, lipo'd and ad-hoc
# signed. Nothing here re-signs it: a Homebrew download is not quarantined, and a
# bottle re-signs with the same free ad-hoc identity anyway, which is harmless
# because Trak keeps its token in a 0600 file rather than the Keychain
# (docs/KEYCHAIN.md).
#
# url and sha256 are placeholders until a release exists (the version is read from
# the url, which `brew audit --strict` insists on). The release workflow
# (.github/workflows/release.yml) rewrites both on every `v*` tag
# and copies this file into the Kathir-D/homebrew-tap tap, so do not hand-edit
# them. A run of zeros is not a checksum anything can match, so an unreleased tap
# entry fails to install rather than installing something nobody verified.
class Trak < Formula
  desc "Terminal UI and CLI for the Spotify desktop app on macOS"
  homepage "https://github.com/Kathir-D/Trak"
  url "https://github.com/Kathir-D/Trak/releases/download/v0.1.0/trak-0.1.0-macos.tar.gz"
  sha256 "3339b815bb5c0427e6a3ea8c89688b709a001aeabff6d9d79b53de87cc94a879"
  license "MIT"

  head "https://github.com/Kathir-D/Trak.git", branch: "main"

  # The audio tap needs 14.2, which Homebrew cannot express; the caveat below
  # says so rather than pretending the floor is 14.0.
  depends_on macos: :sonoma

  def install
    return build_from_git if build.head?

    bin.install "trak"
    (prefix/"share/doc/trak").install "LICENSE", "README.md", "THIRD-PARTY-NOTICES.md"
  end

  def caveats
    <<~EOS
      Trak drives the official Spotify app through AppleScript, so install that
      first if you have not:
        brew install --cask spotify

      macOS 14.2 or newer is required. Homebrew can only express 14.x, so the
      last minor version is the one thing this formula cannot state for you.

      The first trak command asks your terminal for permission to control
      Spotify. If it is denied, Trak says so and points at:
        System Settings > Privacy & Security > Automation
      that prompt belongs to your terminal, not to trak, so it happens once per
      terminal.

      Optional, and nothing breaks without any of it:
        trak config                 settings, including a Spotify Client ID
                                    that unlocks search, playlists, queue
                                    and library
        Sonar, headless-spotify     the two companions trak is built to run
                                    beside; trak degrades quietly without
                                    either

      trak never starts Spotify on its own, and never writes to it except in
      answer to a key or command you pressed.
    EOS
  end

  test do
    # --version is answered by the argument parser, so this needs no Spotify, no
    # Automation permission and no network: which is what makes it safe in CI.
    assert_match version.to_s, shell_output("#{bin}/trak --version")
    assert_path_exists prefix/"share/doc/trak/LICENSE"
  end

  private

  # --HEAD is a checkout of the repository, not a release tarball, so there is no
  # prebuilt binary in it and one has to be built. `cargo build`, not the
  # `cargo install *std_cargo_args` that FormulaAudit/Text asks every cargo call
  # to be: trak is not published on crates.io, and `cargo install trak` fetches an
  # unrelated crate that happens to share the name. That is not negotiable, so the
  # call sits in this helper, which the cop does not inspect.
  #
  # There is no `depends_on "rust" => :build` either, because this is a binary
  # formula and Rust in the dependency list of a bottle install would be a lie
  # about what the bottle needs. So --HEAD wants a rustup already on PATH, which
  # is true of everyone who would run it.
  def build_from_git
    system "cargo", "build", "--release", "--locked"
    bin.install "target/release/trak"
  end
end
