#!/usr/bin/env bash
# MIT License — Copyright (c) 2026 trak contributors (see LICENSE). trak descends
# from shpotify by Harish Narayanan; see THIRD-PARTY-NOTICES.md.
#
# package-release.sh — build the universal release tarball and its checksum.
#
# VERSION is the source of truth: the tag must be v$(cat VERSION) and Cargo.toml
# must agree, or `trak --version` inside the tarball would disagree with the
# filename the release publishes it under. The release workflow checks the same
# three values and refuses to publish when they differ.
#
# Output, both git-ignored, both rebuilt from nothing on every run:
#   dist/trak-<version>-macos.tar.gz   trak, LICENSE, README.md, THIRD-PARTY-NOTICES.md
#   dist/SHA256SUMS.txt                the tarball's sha256, in `shasum -c` format
#
# macOS only, and deliberately dull to run: no network, no sudo, no signing
# identity, nothing written outside dist/ and target/. Nothing here is signed with
# anything paid for — the binary gets an ad-hoc signature, which is free and is
# all macOS 14.2+ asks for (see docs/ARCHITECTURE.md).
#
# Usage: ./scripts/package-release.sh

set -euo pipefail
umask 022

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# The two slices the formula promises. A silent single-arch release is the kind of
# bug nobody notices until an Intel Mac installs it and gets no binary at all.
ARCH_ARM="aarch64-apple-darwin"
ARCH_INTEL="x86_64-apple-darwin"

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

step() {
  printf '\n==> %s\n' "$*"
}

warn() {
  printf 'warning: %s\n' "$*" >&2
}

# Nothing hand-built survives this script, including a failure halfway through: a
# half-staged tree that looks like a release is worse than no release, and the
# tarball only appears once it is complete. The staged binary carries a code
# signature, so leaving it around would also leave an artifact that looks signed
# and installable.
STAGE_ROOT="$REPO_ROOT/dist/stage"
PARTIAL_TARBALL=""
PARTIAL_SUMS=""
cleanup() {
  status=$?
  rm -rf "$STAGE_ROOT"
  if [ -n "$PARTIAL_TARBALL" ]; then
    rm -f "$PARTIAL_TARBALL"
  fi
  if [ -n "$PARTIAL_SUMS" ]; then
    rm -f "$PARTIAL_SUMS"
  fi
  return "$status"
}
trap cleanup EXIT

# ---------------------------------------------------------------- version --

[ -f VERSION ] || die "VERSION is missing; this script must be run from the repository root"
VERSION="$(tr -d ' \t\r\n' < VERSION)"
[ -n "$VERSION" ] || die "VERSION is empty"

# The same expression .github/workflows/ci.yml uses, on purpose: a release must
# not accept a Cargo.toml that CI would have rejected.
CARGO_VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
[ -n "$CARGO_VERSION" ] || die "no version = \"...\" line in Cargo.toml; the package name may have changed"
if [ "$CARGO_VERSION" != "$VERSION" ]; then
  die "VERSION says $VERSION but Cargo.toml says $CARGO_VERSION.
  Bump both in the same commit (docs/RELEASING.md) — trak --version comes from
  Cargo.toml, so only one of them being wrong ships a lying binary."
fi

if [ "$VERSION" = "0.0.0" ]; then
  warn "VERSION is 0.0.0, the unreleased placeholder. This tarball is not publishable."
fi

if git rev-parse -q --verify "refs/tags/v$VERSION" >/dev/null 2>&1; then
  step "tag v$VERSION exists"
else
  # Same hint the sibling projects print, because forgetting the tag is the
  # common way a release ends up untagged.
  step "tag v$VERSION does not exist yet — create it once this passes:"
  printf '  git tag -a v%s -m "trak v%s" && git push origin v%s\n' "$VERSION" "$VERSION" "$VERSION"
fi

# A tarball built from a dirty tree does not match its tag, and nobody notices
# until someone checks out the tag. This is a warning rather than an error so a
# scratch config file does not block a local run.
if [ -n "$(git status --porcelain 2>/dev/null || true)" ]; then
  warn "the working tree has uncommitted changes; the tarball will not match the tag.
  See them with: git status --short"
fi

# ------------------------------------------------------------- toolchain ----

command -v cargo >/dev/null 2>&1 ||
  die "cargo is not on PATH. Rust is installed with rustup: ${CARGO_HOME:-$HOME/.cargo}/bin"

if command -v rustup >/dev/null 2>&1; then
  # Idempotent, and both are asked for explicitly: the host's own target comes with
  # the toolchain, and the other one is the whole point of a universal build.
  rustup target add "$ARCH_ARM" "$ARCH_INTEL" >/dev/null
else
  SYSROOT="$(rustc --print sysroot)"
  for target in "$ARCH_ARM" "$ARCH_INTEL"; do
    [ -d "$SYSROOT/lib/rustlib/$target" ] ||
      die "the $target target is not installed and rustup is not available to add it.
  Install it with: rustup target add $target"
  done
fi

# ----------------------------------------------------------------- build ----

for target in "$ARCH_ARM" "$ARCH_INTEL"; do
  step "building $target"
  # --locked: a release that quietly updates Cargo.lock is a release of code
  # nothing has tested.
  cargo build --release --locked --target "$target"
done

STAGE="$STAGE_ROOT/trak-$VERSION"
mkdir -p "$STAGE"

for target in "$ARCH_ARM" "$ARCH_INTEL"; do
  slice="target/$target/release/trak"
  [ -f "$slice" ] || die "missing $slice — the build for $target did not produce a binary"
done

step "universal binary"
/usr/bin/lipo -create "target/$ARCH_ARM/release/trak" "target/$ARCH_INTEL/release/trak" \
  -output "$STAGE/trak"
chmod 0755 "$STAGE/trak"

# `lipo -archs` prints the short names, sorted so the comparison cannot depend on
# the order lipo happens to list the slices in.
WANT_ARCHS="arm64 x86_64 "
GOT_ARCHS="$(/usr/bin/lipo -archs "$STAGE/trak" | tr ' ' '\n' | LC_ALL=C sort | tr '\n' ' ')"
if [ "$GOT_ARCHS" != "$WANT_ARCHS" ]; then
  die "the merged binary holds [$GOT_ARCHS], expected [$WANT_ARCHS]"
fi
printf 'ok: %s\n' "$(/usr/bin/file -b "$STAGE/trak")"

# ------------------------------------------------------------------ sign ----

# An unsigned arm64 slice is killed by the kernel, so the ad-hoc signature is not
# optional even though we never buy a certificate (docs/ARCHITECTURE.md). It is
# applied to the merged binary rather than to each slice, so one signature covers
# the whole thing; --force is needed because ld(1) has already given the binary a
# signature of its own.
step "ad-hoc signature"
/usr/bin/codesign --force --sign - "$STAGE/trak" ||
  die "codesign failed. On macOS it comes from the Xcode Command Line Tools: xcode-select --install"
/usr/bin/codesign --verify --strict "$STAGE/trak" ||
  die "codesign --verify rejects the binary we just signed"

# Captured rather than piped: `codesign -dv | grep -q` can make codesign die of
# EPIPE, which under `set -o pipefail` would fail a signature that is perfectly
# good. `Signature=adhoc` is what proves there is no paid identity involved;
# `linker-signed` would mean codesign never re-signed anything.
SIGN_INFO="$(/usr/bin/codesign -dv "$STAGE/trak" 2>&1)" || die "codesign -dv failed"
case "$SIGN_INFO" in
  *Signature=adhoc*) ;;
  *) die "the signature is not ad-hoc:
$SIGN_INFO" ;;
esac
case "$SIGN_INFO" in
  *linker-signed*) die "the binary still carries the linker's signature, so nothing was re-signed" ;;
esac
# The CodeDirectory line is the proof in the log: `flags=0x2(adhoc)` on a binary
# this script signed, against `flags=0x20002(adhoc,linker-signed)` on one the
# linker signed by itself.
printf '%s\n' "$SIGN_INFO" | grep -E '^(CodeDirectory|Signature)'

# The one thing a release engineer needs to see: the artifact runs, and it
# reports the version this script was told to build. `--version` is answered by
# the argument parser, so this touches no Spotify, no permission and no network.
REPORTED="$("$STAGE/trak" --version)" ||
  die "the merged binary will not run on this machine: $STAGE/trak --version failed"
if [ "$REPORTED" != "trak $VERSION" ]; then
  die "the binary reports '$REPORTED', expected 'trak $VERSION'"
fi
printf 'ok: %s\n' "$REPORTED"

# ------------------------------------------------------------- tarball ------

# The license and the notices ship with every release, so a missing file is a
# build failure here rather than a licensing problem in somebody's cellar.
for doc in LICENSE README.md THIRD-PARTY-NOTICES.md; do
  [ -f "$doc" ] || die "missing $doc; the tarball must carry it"
  cp "$doc" "$STAGE/$doc"
done

TARBALL_NAME="trak-$VERSION-macos.tar.gz"
mkdir -p dist
PARTIAL_TARBALL="dist/$TARBALL_NAME.partial"
PARTIAL_SUMS="dist/SHA256SUMS.txt.partial"

step "tarball"
# bsdtar writes AppleDouble members and extended attributes unless told not to,
# and both would make the tarball depend on the machine that built it. The
# signature itself lives inside the Mach-O, not in an xattr, so nothing that
# matters is dropped here.
COPYFILE_DISABLE=1 /usr/bin/tar --no-mac-metadata --no-xattrs \
  -czf "$PARTIAL_TARBALL" -C "$STAGE_ROOT" "trak-$VERSION"

# Assert the contents rather than trusting the copy loop: a tarball missing its
# license passes every other check here.
WANT_ENTRIES="$(printf 'trak-%s/\ntrak-%s/LICENSE\ntrak-%s/README.md\ntrak-%s/THIRD-PARTY-NOTICES.md\ntrak-%s/trak\n' \
  "$VERSION" "$VERSION" "$VERSION" "$VERSION" "$VERSION" | LC_ALL=C sort)"
GOT_ENTRIES="$(/usr/bin/tar -tzf "$PARTIAL_TARBALL" | LC_ALL=C sort)"
if [ "$GOT_ENTRIES" != "$WANT_ENTRIES" ]; then
  printf 'error: the tarball holds:\n%s\n' "$GOT_ENTRIES" >&2
  die "the tarball does not hold exactly trak, LICENSE, README.md and THIRD-PARTY-NOTICES.md"
fi

# Renamed, never written in place: a reader must never see a partial tarball.
mv "$PARTIAL_TARBALL" "dist/$TARBALL_NAME"
PARTIAL_TARBALL=""

(cd dist && /usr/bin/shasum -a 256 "$TARBALL_NAME" > SHA256SUMS.txt.partial)
# Written to one side and renamed, so a failed checksum can never leave a stale or
# empty SHA256SUMS.txt for the release to publish.
mv "dist/SHA256SUMS.txt.partial" dist/SHA256SUMS.txt
PARTIAL_SUMS=""
SHA256="$(cut -d' ' -f1 < dist/SHA256SUMS.txt)"
[ -n "$SHA256" ] || die "dist/SHA256SUMS.txt is empty"

# --------------------------------------------------------------- publish ----

step "wrote dist/$TARBALL_NAME"
cat "dist/SHA256SUMS.txt"
printf 'sha256: %s\n' "$SHA256"

# The workflow reads these instead of re-deriving the checksum, so there is one
# number in the pipeline rather than three ways of computing it.
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    printf 'version=%s\n' "$VERSION"
    printf 'sha256=%s\n' "$SHA256"
    printf 'tarball=dist/%s\n' "$TARBALL_NAME"
  } >>"$GITHUB_OUTPUT"
fi

printf '\nverify: (cd dist && shasum -a 256 -c SHA256SUMS.txt)\n'
printf 'publish: gh release create v%s dist/%s dist/SHA256SUMS.txt --title v%s\n' \
  "$VERSION" "$TARBALL_NAME" "$VERSION"
