# Security

## Reporting a problem

Please report a vulnerability privately through GitHub's **Report a vulnerability** button on the
repository's Security tab, not in a public issue. Include what you did, what you expected, and what
happened; there is no need to include a token (see below, and please do not).

## How Trak handles credentials

- **No client secret exists.** Trak uses Authorization Code with PKCE and a user-supplied Client ID,
  so there is nothing secret to ship or to leak from the binary.
- **The token is a `0600` file**, `~/.config/trak/token.json` (`XDG_CONFIG_HOME` is respected), in a
  `0700` directory, written atomically. Trak refuses to read a token file whose mode is looser than
  `0600`. There is no Keychain backend on purpose (it hangs for an unsigned binary; see
  `docs/KEYCHAIN.md`).
- **The token never appears in `config.toml`, in logs, in error messages or in `Debug` output**;
  every type that can hold one prints `[redacted]`, and a test asserts no error notice contains it.
- **The login listener binds `127.0.0.1` on an ephemeral port**, checks the `state` value, and is
  closed as soon as one redirect has been handled.
- **Refresh tokens expire after six months.** Trak warns before then and, if Spotify rejects one,
  deletes it and asks you to log in again.
- Trak talks to Spotify only through the desktop app (AppleScript) and, if you add a Client ID,
  the Web API over HTTPS. It sends nothing anywhere else, except lyric lookups to LRCLIB
  (`[lyrics] enabled = false` turns that off).

To revoke Trak completely: remove it under *Apps* in your Spotify account page, and delete
`~/.config/trak/token.json`.
