#!/bin/sh
# MIT License — Copyright (c) 2026 Trak contributors (see LICENSE). Trak descends
# from shpotify by Harish Narayanan; see THIRD-PARTY-NOTICES.md.
#
# install.sh — install the prebuilt trak binary from a GitHub release, without
# Homebrew (TODO 9.9). Homebrew is the first-class path; this is the second.
#
#   curl -fsSL https://raw.githubusercontent.com/Kathir-D/Trak/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/Kathir-D/Trak/main/install.sh | sh -s -- --uninstall
#
# It downloads the same tarball and SHA256SUMS.txt the formula uses, refuses to
# install anything whose checksum does not match, and copies one file: trak.
#
# What it deliberately does not do:
#   - sudo. /usr/local/bin is used only if you can already write to it; otherwise
#     ~/.local/bin. A silent sudo inside a piped script is a password prompt you
#     did not ask for.
#   - touch quarantine. curl does not set com.apple.quarantine, so there is
#     nothing to strip, and adding or removing it would be a Gatekeeper trick.
#   - launch Spotify, or run trak beyond `trak --version` (docs/COMPAT.md rule 2).
#   - remove your settings or login (~/.config/trak) on --uninstall; it installed
#     neither.
#
# Environment:
#   TRAK_VERSION       version to install (default: the latest release)
#   TRAK_INSTALL_DIR   where trak goes (default: /usr/local/bin if writable,
#                      else ~/.local/bin)
#   TRAK_BASE_URL      where the release files live; the tests point this at a
#                      file:// mirror (default: the GitHub release for the version)

set -eu

REPO="Kathir-D/Trak"
MIN_MAJOR=14
MIN_MINOR=2

say() {
  printf '%s\n' "$*"
}

die() {
  printf 'trak install: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<EOF
Install trak, a terminal UI and CLI for the Spotify desktop app on macOS.

usage: install.sh [--uninstall] [--help]

  (no flags)    download the latest release, verify its sha256, install trak
  --uninstall   remove the trak this script installed (settings are kept)
  --help        this text

environment:
  TRAK_VERSION       install this version instead of the latest (e.g. 0.1.0)
  TRAK_INSTALL_DIR   install here (default: /usr/local/bin if writable,
                     else ~/.local/bin)

Homebrew is the other way in: brew install kathir-d/tap/trak
EOF
}

# The receipt is how --uninstall removes exactly what was installed and nothing
# else: one path, written after the binary is in place. It lives under the XDG
# data dir rather than next to the binary, so /usr/local/bin gets one file.
receipt_path() {
  printf '%s/trak/install-sh-receipt\n' "${XDG_DATA_HOME:-$HOME/.local/share}"
}

# Homebrew owns its own links. Overwriting one would leave brew believing it
# still manages a file that is now ours, and removing one would break brew.
is_homebrew_link() {
  [ -L "$1" ] || return 1
  case "$(readlink "$1")" in
    *Cellar/*) return 0 ;;
    *) return 1 ;;
  esac
}

uninstall() {
  receipt="$(receipt_path)"
  if [ ! -f "$receipt" ]; then
    say "Nothing to remove: this script has not installed trak for $(id -un)."
    say "(A Homebrew install is removed with: brew uninstall trak)"
    return 0
  fi
  target="$(head -n 1 "$receipt")"
  case "$target" in
    /*/trak) ;;
    *) die "the receipt at $receipt names '$target', which is not a trak path; remove it by hand" ;;
  esac
  if [ -e "$target" ] || [ -L "$target" ]; then
    if is_homebrew_link "$target"; then
      die "$target now belongs to Homebrew; use: brew uninstall trak"
    fi
    rm -f "$target" || die "could not remove $target (permission?)"
    say "Removed $target"
  else
    say "$target is already gone."
  fi
  rm -f "$receipt"
  rmdir "$(dirname "$receipt")" 2>/dev/null || true
  say "Settings and any Spotify login in ~/.config/trak were left alone."
}

check_macos() {
  [ "$(uname -s)" = "Darwin" ] || die "trak runs on macOS only (this is $(uname -s))."
  product="$(sw_vers -productVersion 2>/dev/null)" ||
    die "could not read the macOS version (sw_vers failed)."
  major="$(printf '%s' "$product" | cut -d. -f1)"
  minor="$(printf '%s' "$product" | cut -d. -f2 -s)"
  minor="${minor:-0}"
  case "$major$minor" in
    *[!0-9]*|'') die "could not understand the macOS version '$product'." ;;
  esac
  # 14.2 is where Core Audio process taps start, which the visualizer is built on
  # (docs/SPEC.md §2); the formula says the same in its caveats.
  if [ "$major" -lt "$MIN_MAJOR" ] ||
    { [ "$major" -eq "$MIN_MAJOR" ] && [ "$minor" -lt "$MIN_MINOR" ]; }; then
    die "trak needs macOS $MIN_MAJOR.$MIN_MINOR or newer; this Mac runs $product."
  fi
}

latest_version() {
  # The API answers JSON, and this script has no JSON parser to lean on; the tag
  # name is a flat string field, so a sed over it is enough and fails closed.
  json="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest")" ||
    die "could not ask GitHub for the latest release. Set TRAK_VERSION=x.y.z to skip the lookup."
  tag="$(printf '%s\n' "$json" | sed -n 's/^ *"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
  [ -n "$tag" ] || die "GitHub's latest-release reply carried no tag_name."
  printf '%s\n' "${tag#v}"
}

choose_dir() {
  if [ -n "${TRAK_INSTALL_DIR:-}" ]; then
    printf '%s\n' "$TRAK_INSTALL_DIR"
  elif [ -d /usr/local/bin ] && [ -w /usr/local/bin ]; then
    printf '%s\n' /usr/local/bin
  else
    printf '%s\n' "$HOME/.local/bin"
  fi
}

install_trak() {
  check_macos
  command -v curl >/dev/null 2>&1 || die "curl is required."
  command -v shasum >/dev/null 2>&1 || die "shasum is required."

  version="${TRAK_VERSION:-}"
  [ -n "$version" ] || version="$(latest_version)"
  version="${version#v}"
  case "$version" in
    [0-9]*) ;;
    *) die "'$version' is not a version number." ;;
  esac
  case "$version" in
    *[!0-9A-Za-z.+-]*) die "'$version' is not a version number." ;;
  esac

  base="${TRAK_BASE_URL:-https://github.com/$REPO/releases/download/v$version}"
  tarball="trak-$version-macos.tar.gz"

  tmp="$(mktemp -d "${TMPDIR:-/tmp}/trak-install.XXXXXX")" || die "could not make a temporary directory."
  # The trap is set before anything lands in it, so an interrupted download leaves
  # nothing behind.
  trap 'rm -rf "$tmp"' EXIT
  trap 'exit 130' INT TERM

  say "Downloading trak $version"
  curl -fsSL -o "$tmp/$tarball" "$base/$tarball" ||
    die "could not download $base/$tarball — is $version a published release?"
  curl -fsSL -o "$tmp/SHA256SUMS.txt" "$base/SHA256SUMS.txt" ||
    die "could not download $base/SHA256SUMS.txt"

  # Check only the tarball's own line, so a sums file that one day lists more
  # assets cannot fail the check for a file this script never downloaded.
  grep "  $tarball\$" "$tmp/SHA256SUMS.txt" >"$tmp/expected.txt" ||
    die "SHA256SUMS.txt has no line for $tarball; refusing to install an unverified binary."
  if ! (cd "$tmp" && shasum -a 256 -c expected.txt >/dev/null 2>&1); then
    die "checksum mismatch for $tarball. Nothing was installed.
  expected: $(cut -d' ' -f1 <"$tmp/expected.txt")
  got:      $(shasum -a 256 "$tmp/$tarball" | cut -d' ' -f1)"
  fi
  say "Checksum OK"

  tar -xzf "$tmp/$tarball" -C "$tmp" || die "could not unpack $tarball."
  bin="$tmp/trak-$version/trak"
  [ -f "$bin" ] || die "the tarball has no trak-$version/trak; refusing to guess."

  dir="$(choose_dir)"
  mkdir -p "$dir" || die "could not create $dir"
  [ -w "$dir" ] ||
    die "$dir is not writable. Pick another place with TRAK_INSTALL_DIR=..., e.g. TRAK_INSTALL_DIR=\$HOME/.local/bin"
  dest="$dir/trak"
  if is_homebrew_link "$dest"; then
    die "$dest is Homebrew's trak. Upgrade it with brew upgrade trak, or uninstall it first."
  fi

  # Copy beside the destination and rename over it, so a running trak keeps its
  # old inode and a failure halfway cannot leave a truncated binary on PATH.
  cp "$bin" "$dir/.trak.partial.$$" || die "could not write to $dir"
  chmod 0755 "$dir/.trak.partial.$$"
  mv -f "$dir/.trak.partial.$$" "$dest" || {
    rm -f "$dir/.trak.partial.$$"
    die "could not install $dest"
  }

  receipt="$(receipt_path)"
  if ! { mkdir -p "$(dirname "$receipt")" && printf '%s\n' "$dest" >"$receipt"; }; then
    say "warning: could not write $receipt, so --uninstall will not find this install."
  fi

  say "Installed $("$dest" --version) to $dest"
  case ":$PATH:" in
    *":$dir:"*) ;;
    *) say "Note: $dir is not on your PATH. Add it with: echo 'export PATH=\"$dir:\$PATH\"' >> ~/.zprofile" ;;
  esac
  say "Run trak for the TUI, trak status for a one-liner. It is ad-hoc signed, not notarized."
}

case "${1:-}" in
  -h|--help) usage ;;
  --uninstall) uninstall ;;
  '') install_trak ;;
  *)
    usage >&2
    exit 2
    ;;
esac
