# Trak — releasing (TODO 9.1, 9.2, 9.3, 9.7)

A release is **one tag** and **two CI jobs**. Everything else in this file is what
you do before and after that tag, in order, so that a half-finished release at 1am
is impossible to mistake for a finished one.

What exists today:

| Piece | File | What it is |
| --- | --- | --- |
| Packaging | `scripts/package-release.sh` | Universal binary, ad-hoc signed, tarball + `SHA256SUMS.txt` in `dist/` |
| Automation | `.github/workflows/release.yml` | Runs on `v*` tags: gate, package, release, then the tap commit |
| Formula | `Formula/trak.rb` | Placeholders until a release; CI rewrites `url` and `sha256` |
| Tap | `github.com/Kathir-D/homebrew-tap` | `Formula/trak.rb`, written by this repo's release workflow and nothing else |
| curl installer | `install.sh` | Installs the same tarball without Homebrew, after a sha256 check (§10, TODO 9.9) |

**No Apple Developer account, no Developer ID, no paid signing, no notarisation.**
The binary is signed with `codesign -s -`, which is free. See §6 for why that is
also the right answer for a bottled formula.

## 1. Before you tag

Work on `main`, one conventional commit per step, and let CI go green first. A tag
on a red build is worse than no tag: the gate is the only thing standing between a
broken commit and a published sha256 that points at it.

```sh
git switch main && git pull --ff-only
cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test
```

Then, in this order:

1. **Bump `VERSION` and `Cargo.toml` in the same commit.** `VERSION` is the source
   of truth and the tag must be `v$(cat VERSION)`; `Cargo.toml` is what
   `trak --version` actually prints. CI and the release workflow both refuse to
   continue if they disagree, which is the point.

   ```sh
   printf '0.1.0\n' > VERSION
   # and in Cargo.toml, the [package] version = "0.1.0"
   ```

2. **Write the changelog entry.** `CHANGELOG.md` is in
   [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format: move what is
   under `## [Unreleased]` into a new `## [x.y.z] - date` section below it and
   update the compare links at the bottom. Compare against the previous tag so the
   entry describes what changed in *this* release:

   ```sh
   PREV="$(git describe --tags --abbrev=0 2>/dev/null || echo v0.0.0)"
   git log --oneline "$PREV"..HEAD
   ```

3. **Dry-run the packaging locally.** It needs no network, no `sudo` and no
   signing identity, and it writes only to `dist/` and `target/`.

   ```sh
   ./scripts/package-release.sh
   ```

   It fails loudly rather than shipping something odd: on a version mismatch, on a
   missing slice, on a binary that is not both architectures, on a signature that
   is not ad-hoc, on a tarball missing its license, on a binary whose `--version`
   does not match. It leaves no staging directory behind, on success or failure.

4. **Smoke-test the formula against that local tarball.** This is TODO 9.3's
   `Done when`, and it is the only chance to catch a formula that cannot install.

   ```sh
   V="$(cat VERSION)"
   SHA="$(shasum -a 256 "dist/trak-$V-macos.tar.gz" | cut -d' ' -f1)"
   cp Formula/trak.rb /tmp/trak.rb.orig
   sed -i '' -e "s|^  url .*|  url \"file://$PWD/dist/trak-$V-macos.tar.gz\"|" \
          -e "s|^  version .*|  version \"$V\"|" \
          -e "s|^  sha256 .*|  sha256 \"$SHA\"|" Formula/trak.rb
   brew install --formula ./Formula/trak.rb
   brew test trak
   git restore Formula/trak.rb      # the committed formula must keep the release URL
   ```

5. **Commit and push.** Then watch CI: <https://github.com/Kathir-D/Trak/actions>.

## 2. Tag

Only once `main` is green and the local dry run has passed.

```sh
git tag -a "v$(cat VERSION)" -m "trak v$(cat VERSION)"
git push origin main "v$(cat VERSION)"      # the tag push is what starts the release
```

The tag must point at the commit that carries the bumped `VERSION` and
`Cargo.toml`. **Never move a tag that has been pushed**: anyone who has already
fetched it keeps the old one, and the release job republishes assets under a name
that is already cached, with a different sha256 (§9).

## 3. What CI does

`.github/workflows/release.yml`, on any `v*` tag, in two jobs.

**`release`** — the job that produces the artifact:

1. Checkout, the same toolchain and the same action versions as `ci.yml`.
2. **The tag, `VERSION` and `Cargo.toml` must agree.** Any disagreement is an
   error before anything is built.
3. **The gate**: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D
   warnings`, `cargo test`. The release build itself happens in the packaging
   script.
4. **`./scripts/package-release.sh`**: builds `aarch64-apple-darwin` and
   `x86_64-apple-darwin` with `--release --locked`, `lipo`s them together, checks
   both architectures are present, ad-hoc signs the merged binary, checks the
   signature is ad-hoc and not the linker's, runs `--version` on it, tars
   `trak`, `LICENSE`, `README.md` and `THIRD-PARTY-NOTICES.md` without extended
   attributes, and writes `dist/SHA256SUMS.txt`.
5. **Publishes the GitHub release** with the tarball and `SHA256SUMS.txt`, and
   notes generated from the merged commits. It fails if the glob matches nothing,
   so a release with no tarball cannot exist.

**`formula`** — the job that tells Homebrew about it:

1. Takes the version and the sha256 the packaging script reported, as job
   outputs. The checksum in the formula is the same number that was published,
   not a second calculation of it.
2. Rewrites `url` and `sha256` in `Formula/trak.rb` in place (the version comes from
   the url; a separate `version` line fails `brew audit --strict`), refusing
   to continue if either line is not found or either value is not the
   right shape.
3. Commits the rewritten formula back to `main` here (rebased onto whatever
   `main` is now), the way headless-spotify keeps its cask in step, so the file in
   this repository always says what the tap says.
4. Copies that file into `Kathir-D/homebrew-tap` through the `TAP_DEPLOY_KEY`
   deploy key and commits it. It owns that one file in the tap, which is the tap's
   one-writer-per-file rule; the tap README is not touched.

The two jobs are separate on purpose: a tap failure leaves a good release
published, and the sha256 crosses the boundary as an output rather than as a file
on a runner that no longer exists.

## 4. Verify the published release

Do this by hand once per release. It is five minutes and it is the only thing that
proves the artifact a user downloads is the one you built.

```sh
V="$(cat VERSION)"
base="https://github.com/Kathir-D/Trak/releases/download/v$V"
curl -LO "$base/trak-$V-macos.tar.gz"
curl -LO "$base/SHA256SUMS.txt"

shasum -a 256 -c SHA256SUMS.txt          # must say OK
tar -tzf "trak-$V-macos.tar.gz"         # trak, LICENSE, README.md, THIRD-PARTY-NOTICES.md
tar -xzf "trak-$V-macos.tar.gz"
cd "trak-$V"

file trak                                # "universal binary with 2 architectures"
lipo -archs trak                         # arm64 x86_64
codesign -dv trak 2>&1 | grep -E '^(CodeDirectory|Signature)'
#   CodeDirectory v=20500 ... flags=0x2(adhoc) ... location=embedded
#   Signature=adhoc
codesign --verify --strict trak && echo "signature verifies"
xattr -l trak                            # must print nothing: no quarantine flag
./trak --version                         # trak $V
cd ../..
```

`flags=0x2(adhoc)` is the line that matters. A linker-signed binary reports
`flags=0x20002(adhoc,linker-signed)`, which means nothing re-signed it after the
merge — that is the difference TODO 1.8 turned on, so check it rather than assume
it.

Then install it the way a user would, from the tap (§7): `brew install
kathir-d/tap/trak` and `brew test trak`.

## 5. What a human still has to do

| Step | Who | Notes |
| --- | --- | --- |
| Add the `TAP_DEPLOY_KEY` secret | **[owner]**, once | A deploy key with write access to `Kathir-D/homebrew-tap` and nothing else. Until it exists the release publishes and the tap does not move. |
| Watch the run and read the log | you | The `release` job's `sha256:` line is the number that matters. |
| Verify the artifact | you | §4, in full. |
| Add the row to the tap README | **[owner]** | TODO 9.4. The workflow owns `Formula/trak.rb` and nothing else, so the README table is a hand edit — or run prompt 3 in `docs/AGENT-PROMPTS.md`. |
| `brew audit --strict --online kathir-d/tap/trak` and `brew style` | you, before the announcement | TODO 9.5. A checksum complaint on the *unreleased* formula is expected: the placeholders are meant not to install. |
| Fresh-machine test | you | TODO 9.6, §7. |
| Approve the release | the agent, by owner override | TODO 9.8 (changed 2026-10-01): an agent may tag once phases 2–7 and 9 pass and every pre-tag check in §1 is green; real audio (8.3/8.5) and phase 10 do not block. |
| Check the curl installer against the real release | you | §10, once the release is up. |

## 6. Signing, and why bottles are not a problem

The formula installs a binary that was signed once, at package time. It does not
re-sign, and there is no `system "codesign"` and no quarantine handling in it,
because there is nothing to fix:

- **Homebrew does not quarantine what it downloads.** The `com.apple.quarantine`
  attribute is set by browsers and a few apps, not by `brew`, so a formula install
  produces no Gatekeeper prompt. (This is about formulae: a *cask* is deliberately
  quarantined so Gatekeeper checks the app, which is why the tap's other entries
  have to deal with the flag.) The packaging script also strips extended attributes
  from the tarball (`tar --no-xattrs --no-mac-metadata`), so nothing can carry a
  quarantine flag into a cellar either.
- **Bottling does not change the signature.** Homebrew re-signs a Mach-O file only
  when it has itself modified one — stripping it, rewriting a dylib path, or a
  `codesign` install step. Trak is one self-contained executable with no dylib
  references to rewrite, so it is installed exactly as it was signed. If a future
  change ever did make Homebrew re-sign it, the re-sign would be ad-hoc
  (`codesign -s -`) and therefore still free.
- **A changing code identity is the one thing that does matter, and Trak is immune
  to it.** Every release is a new ad-hoc identity, so every `brew upgrade` is a new
  identity too — which is the exact case TODO 1.8 measured as a *hang* for a
  Keychain item, because a keychain item is readable only by the binary that
  created it. Trak keeps its token in a `0600` file (`docs/KEYCHAIN.md`), which
  survives a re-sign, a re-link and an upgrade. That decision is what makes the
  no-paid-signing distribution safe, and it is the reason the formula does not
  need to sign anything itself.
- **The ad-hoc signature is not optional.** An unsigned arm64 slice is killed by
  the kernel, so `codesign -s -` runs on the merged binary every time. It is free,
  and it is the only signing Trak does or will do.

If a bottle ever becomes the distribution path rather than the release tarball,
nothing above changes; the ad-hoc re-sign is already accounted for.

## 7. Fresh-machine test (TODO 9.6)

After `brew install kathir-d/tap/trak` on a clean user, a VM, or immediately after
`brew uninstall trak`:

```sh
xattr -l "$(which trak)"      # nothing: no quarantine attribute
trak --version                # the released version, no Gatekeeper dialog
brew test trak                # passes with no Spotify and no network
trak status                   # the Automation prompt appears here, once, for the terminal
trak                          # the TUI
```

`Done when`: no Gatekeeper prompt, no "cannot be opened because the developer
cannot be verified", no manual steps, and the Automation prompt is the only one.
Record what you saw, with the date, in the log at the end of this file.

## 8. When a step fails

| What you see | What it means | What to do |
| --- | --- | --- |
| `::error::tag v0.1.0 != v0.1.1` | The tag does not match `VERSION`. | Nothing was published and nobody has the tag, so deleting it is safe: `git tag -d v0.1.0`, `git push --delete origin v0.1.0`, then re-tag or bump. |
| `error: VERSION says … but Cargo.toml says …` | The two were not bumped together. | Nothing was published. Fix both in one commit and re-tag. |
| `error: the merged binary holds [arm64], expected [arm64 x86_64]` | One slice did not build. | The build log above it says which. Nothing was published. |
| `error: the signature is not ad-hoc` or `still carries the linker's signature` | `codesign` did not do its job, usually a broken Command Line Tools install. | `xcode-select -p` and `xcode-select --install` on the runner. |
| `error: the binary reports 'trak 0.0.0', expected 'trak 0.1.0'` | A stale binary in `target/`. | Re-run the job; the checkout is fresh each time, so this points at a `VERSION`/`Cargo.toml` mismatch instead. |
| Red `fmt` / `clippy` / `test` in the gate | The tag points at a commit CI rejected. | **No release was published.** Fix it on `main`, then cut a new tag. Do not re-point the old one. |
| `TAP_DEPLOY_KEY is not set` | The secret is missing. | The release **is** up. Add the secret, then re-run the `formula` job. |
| `git push` to the tap is rejected | Branch protection on the tap, or the deploy key lost write access. | The release is up. Fix the tap's protection or the key, then re-run `formula`. |
| `expected exactly one sha256 line in Formula/trak.rb` | The formula's layout changed under the workflow. | The release is up and the tap is untouched. Update the substitution in `release.yml` to match the new layout and re-run `formula`. |
| `brew audit` complains about the checksum | You are auditing the unreleased formula. | Expected. The placeholders are not installable on purpose. |

## 9. Yanking a bad release

A release is a tag, an asset, and a commit in a tap that points at that asset's
checksum. Fixing one of them does not fix the others, so the honest answer for a
bad *binary* is a new version:

```sh
gh release delete "v$(cat VERSION)"          # the release page and its assets
git push --delete origin "v$(cat VERSION)"  # the tag
```

Then bump the patch version, tag it, and let the workflow publish and update the
tap. The tap's old commit stays in the tap's history, which is correct: it is what
was true at the time.

Do **not** rebuild a published tarball and `gh release upload --clobber` it. The
packaging is not byte-reproducible — the ad-hoc signature covers the built bytes
and gzip stamps a timestamp into the archive — so a rebuild has a different
sha256, and the formula in the tap would then point at a checksum that no longer
matches the asset. A re-run of the `release` job for a tag that already published
replaces the assets for the same reason, so the tap's checksum goes stale: if you
have to re-run it, re-run `formula` too, and understand that the two commits will
differ.

For a bad *formula* with a good binary, fix `Formula/trak.rb` here, commit it, and
have the **[owner]** push that one file to the tap. (The `formula` job also commits
the bumped release lines back to `main` here, so pull before editing the file.) The workflow only runs on tags,
and the tap's one-writer rule means the file is not edited from two places.

## 10. The curl installer (TODO 9.9)

`install.sh` at the repository root is the second way in, for people without
Homebrew. It needs nothing from a release beyond what the workflow already
publishes: it downloads `trak-<v>-macos.tar.gz` and `SHA256SUMS.txt` from the
GitHub release, so there is nothing extra to build or upload, and the raw URL on
`main` is the one people pipe into `sh`:

```sh
curl -fsSL https://raw.githubusercontent.com/Kathir-D/Trak/main/install.sh | sh
curl -fsSL https://raw.githubusercontent.com/Kathir-D/Trak/main/install.sh | sh -s -- --uninstall
TRAK_VERSION=0.1.0 TRAK_INSTALL_DIR="$HOME/bin" sh install.sh     # a pinned version, a chosen dir
```

What it promises, each one covered by `tests/install_sh.rs` against a `file://`
mirror (`TRAK_BASE_URL` points it there):

- macOS 14.2 or newer, or it stops before downloading anything;
- the tarball's line in `SHA256SUMS.txt` must verify with `shasum -a 256 -c`, or
  nothing is installed;
- `trak` goes to `$TRAK_INSTALL_DIR`, else `/usr/local/bin` if it is already
  writable, else `~/.local/bin` (with a one-line PATH hint when that is not on
  `PATH`). **It never runs `sudo`**;
- it never overwrites a Homebrew-managed `trak` (a symlink into a `Cellar`);
- `--uninstall` removes the one file named in its receipt
  (`${XDG_DATA_HOME:-~/.local/share}/trak/install-sh-receipt`) and leaves
  `~/.config/trak` alone;
- quarantine: `curl` does not set `com.apple.quarantine`, so there is nothing to
  strip and the script does not touch it. `xattr -l "$(command -v trak)"` prints
  nothing after a curl install, the same as after a Homebrew one.

After a release, run it for real once and record the output in §11:

```sh
curl -fsSL https://raw.githubusercontent.com/Kathir-D/Trak/main/install.sh | sh
trak --version && xattr -l "$(command -v trak)" && trak status
curl -fsSL https://raw.githubusercontent.com/Kathir-D/Trak/main/install.sh | sh -s -- --uninstall
```

## 11. Release log

One row per release, filled in from §4 and §7.

| Version | Date | sha256 (first 12) | Tap commit | Fresh-machine result |
| --- | --- | --- | --- | --- |
| 0.1.0 | 2026-10-02 | `3339b815bb5c` | `92359db` | Workflow green. `brew audit --strict --online` failed: `version "0.1.0"` is redundant with the URL. Superseded by 0.1.1; the binary is the same. |
| 0.1.1 | 2026-10-02 | `5b48141e9d57` | `b19d4b9` | Both install paths below, verbatim. |

### 0.1.1 fresh install, 2026-10-02 (the owner's Mac, Apple silicon, macOS 27)

Homebrew (`brew uninstall trak`, then `brew update`, then):

```text
$ brew install kathir-d/tap/trak
$ trak --version
trak 0.1.1
$ xattr -l "$(which trak)"          # nothing printed: no quarantine attribute
$ ls /Applications | grep -ic trak
0
$ lipo -archs /opt/homebrew/bin/trak
x86_64 arm64
$ brew audit --strict --online kathir-d/tap/trak    # exit 0, no output
$ brew test kathir-d/tap/trak                       # exit 0
```

`trak` is the only file in `/opt/homebrew/bin`; the Cellar holds the binary and
`share/doc/trak`. macOS also resolves `Trak` to it, because the default volume is
case-insensitive.

curl (after `brew uninstall trak`, so the default location is free):

```text
$ curl -fsSL https://raw.githubusercontent.com/Kathir-D/Trak/main/install.sh | sh
Downloading trak 0.1.1
Checksum OK
Installed trak 0.1.1 to /Users/kathirdev/.local/bin/trak
Run trak for the TUI, trak status for a one-liner. It is ad-hoc signed, not notarized.
$ trak --version
trak 0.1.1
$ xattr -l ~/.local/bin/trak         # nothing printed
$ curl -fsSL .../install.sh | sh -s -- --uninstall
Removed /Users/kathirdev/.local/bin/trak
Settings and any Spotify login in ~/.config/trak were left alone.
```

Not run: a Mac that has never had Homebrew or a Spotify Automation grant, so the
first-run permission prompt (§7's `trak status` step) was not seen from scratch
**[owner]**.
