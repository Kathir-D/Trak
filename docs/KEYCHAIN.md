# Trak — Keychain vs. a `0600` file (TODO 1.8, risk R3)

**Measured 2026-09-29 on the owner's machine** (macOS 27.0, arm64) with
`spikes/keychain`, a Rust binary using the `security-framework` crate.

## Verdict

**Use a `0600` file. Do not use the Keychain.**

R3 is confirmed, and it is worse than the risk table said. The Keychain is not
merely "likely to re-prompt after every upgrade" — on this machine **any keychain
read by a process other than the one that created the item blocks forever** in an
unattended session. Since `scripts/package-release.sh` (TODO 9.1) re-signs and
re-links the binary on every release, "every upgrade" means "every single upgrade",
and the failure mode is a hang, not a prompt.

## What was measured

Two builds of the same program with different embedded strings, so each has a
different code identity:

- **v1** — plain `cargo build --release`. `codesign -dv` reports
  `flags=0x20002(adhoc,linker-signed)`, `Signature=adhoc`.
- **v2** — the same source re-signed with `codesign --force -s -`, which is what
  TODO 9.1's release script does.
- **v3** — a third build, linker-signed, different identity again.
- **`security`** — Apple's own CLI, used as a control.

Each operation was run under a hard timeout, because a keychain authorization
dialog is a modal GUI panel that nobody is present to dismiss.

| # | Operation | Result |
| --- | --- | --- |
| A | v1 stores a new item | `STORED`, rc=0, **no prompt** |
| B | v1 reads its own item | `READ`, rc=0, **no prompt** |
| C | v2 (re-signed) reads v1's item | **blocks** (>12 s) |
| D | v2 (re-signed) stores a *new* item | **blocks** (>12 s) |
| E | v3 (linker-signed, new identity) stores a new item | `STORED`, rc=0, no prompt |
| F | v3 (different identity) reads v1's item | **blocks** |
| G | `security` (Apple-signed) reads v1's item | **blocks** |

The block is not a permanent hang, and the distinction matters. macOS **caches the
authorization decision per (item, code identity)**:

- the **first** time a given identity touches a foreign item, it raises the dialog
  and the call blocks (rows C, D, F, G);
- every **subsequent** call by that *same* identity returns an error immediately,
  with no dialog.

Neither returns the secret, so the item is unreadable either way. What makes this
fatal for Trak is that the blocking case is always the one it hits: every release
is a brand-new identity, so there is never a cached decision to fall back on. If a
future reader of this doc only ever sees the second behaviour, that is an artifact
of testing the same binary twice, not evidence the problem went away.

The Keychain itself was verified healthy throughout, so none of these blocks are a
locked-keychain artefact:

```
$ security show-keychain-info ~/Library/Keychains/login.keychain-db
Keychain "…/login.keychain-db" no-timeout
$ security add-generic-password -s trak-probe -a probe -w p -U && \
  security find-generic-password -s trak-probe -a probe -w
p
rc=0            # instant, no prompt
```

`CoreServicesUIAgent` and `SecurityAgent` are both running while a foreign binary
blocks, which is what an undismissable authorization panel looks like from the
outside.

## Why this happens

`security_framework::passwords::set_generic_password` calls plain `SecItemAdd`
with **no `kSecAttrAccess`**, so the new item gets the default ACL — which trusts
*the creating binary, by code identity*. The item carries no creator attribute
(`"crtr"<uint32>=<NULL>` in `security dump-keychain`), so the binding is not a
path or a bundle id: it is the code hash.

Therefore the invariant is:

> A keychain item is silently readable **only** by the exact binary that created
> it. Every other process — a newer build, an older build, even Apple's own
> `security` — fails to read it, either by raising a GUI dialog (the first time
> that identity tries) or by returning an error (thereafter).

Row E is the one nuance worth keeping: a binary that *creates* its own item is
fine, because there is nothing to authorize. So the Keychain looks like it works
during development. The failure only shows up on the first launch after the
binary changes, which is exactly the moment nobody is watching.

### Why Trak cannot live with this

The distribution model guarantees the identity changes. `scripts/package-release.sh`
(TODO 9.1) builds both architectures, `lipo`s them together, and runs
`codesign --force -s -`. Every release is therefore a new code hash, so:

1. Trak v1.0.0 stores a refresh token, created by binary A.
2. `brew upgrade` installs Trak v1.1.0, which is binary B.
3. v1.1.0 reads the token → blocks on a GUI panel → **Trak hangs on startup**.

The hang is the worst possible failure for a TUI: it is not an error the user can
read, and it cannot be recovered from without `kill -9`. Detecting it would mean a
timeout around a keychain call, and a call blocked in Security.framework cannot
be cancelled — the thread leaks. And because the Keychain path is the
*default*, the failure lands on the upgrade path, which is the one every user
takes.

A secondary cost: because `security` is also blocked (row G), Trak cannot inspect
or migrate its own item with the standard tool, so there is no escape hatch
short of a GUI click.

## The decision

**A `0600` file, not the Keychain.**

| | Keychain | `0600` file |
| --- | --- | --- |
| Survives a Homebrew upgrade | **no** — prompts, then hangs | **yes** |
| Survives a rebuild | no | yes |
| Fails closed on a shared machine | yes (OS-enforced) | yes, if the mode is really `0600` |
| Recoverable without a GUI | no | yes — delete the file |
| Needs a GUI to be usable | effectively yes | no |

The file's only real weakness is that `0600` is advisory, so the guarantee depends
on Trak creating the file with the right mode and not loosening it. That is
testable, so TODO 5.1 and 7.4 must assert the mode in their tests:

- create with `OpenOptions::mode(0o600)` and verify with `metadata().permissions().mode() & 0o777 == 0o600`;
- refuse to read the token if the file is group- or world-accessible, rather than
  silently proceeding;
- write atomically (temp file in the same directory, then `rename`) so a crash
  mid-write cannot leave a truncated or world-readable token;
- location `~/.config/trak/token.json`, respecting `XDG_CONFIG_HOME`, alongside
  `config.toml`, with the directory itself created `0700`.

The directory on this machine is already correct:
`drwx------ ~/.config` (0700), and a probe file was created and read back as
`-rw-------` (0600).

## What this changes elsewhere

- `docs/SPEC.md` §2 ("Token storage: macOS Keychain first") and §6 are updated by
  the commit that lands this: the file is the default, and the Keychain is not
  offered as an option, because offering it would mean offering a hang.
- **R3 in `TODO.md`** is marked resolved with this measurement.
- TODO 7.4 (`Store` trait, both backends) is reduced to **one** backend behind the
  trait. Keep the trait — it is what makes the token store fakeable in tests — but
  do not build a second implementation that nothing will use.
- `docs/WEB-API.md` §6 is unaffected but still applies: a refresh token only lives
  **6 months** regardless of where it is stored, so the reconnect flow is needed
  either way.

## Re-checking this yourself

**The 1.8 check is opt-in, on purpose.** Reproducing this provokes a *real*
macOS keychain authorization dialog that asks for your login password. Run it
only when you are sitting at the machine:

```sh
./spikes/verify.sh              # everything except the keychain
./spikes/verify.sh --keychain   # includes it; will show dialogs
```

It uses a per-run service name (`trak-verify-$$`) and a `trap` on `EXIT INT TERM`,
so it deletes every item it created even if you Ctrl-C it partway through. This
exists because an earlier version used a fixed name and left items and stacked-up
dialogs behind.

## Reproducing

```sh
cd spikes/keychain
cargo build --release
B=./target/release/keychain-spike

# store with this build, then read with a *re-signed* copy of it
$B store probe acct hello
cp $B /tmp/ks-v2 && codesign --force -s - /tmp/ks-v2
/tmp/ks-v2 read probe acct        # blocks: the item is bound to the first binary's hash
security delete-generic-password -s probe -a acct
```

Rebuild the crate with a different embedded string to get a fresh linker-signed
identity without running `codesign` at all; that alone is enough to reproduce
(rows E/F).
