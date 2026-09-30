# spikes/keychain — TODO 1.8

Answers R3: **can an ad-hoc-signed binary store and read a secret in the macOS
Keychain without prompting, and does that survive a rebuild?**

Short answer: **no, and it is worse than the risk table assumed.** Any keychain
read by a process other than the one that created the item blocks on a GUI
authorization panel, and a re-signed binary blocks even when *creating* an item.
Because `scripts/package-release.sh` re-signs on every release, that means a hang
on the first launch after every Homebrew upgrade.

Findings, with the full experiment table, are in `docs/KEYCHAIN.md`. The decision
is a `0600` file instead of the Keychain.

```sh
cargo build --release
B=./target/release/keychain-spike
$B store probe acct hello
cp $B /tmp/ks-v2 && codesign --force -s - /tmp/ks-v2
/tmp/ks-v2 read probe acct     # blocks
security delete-generic-password -s probe -a acct
```

Run the blocking read under a timeout, or the shell will hang with it: a keychain
authorization panel is modal and cannot be dismissed from an unattended session.

The program takes `store|read|delete <service> <account> [secret]`.
