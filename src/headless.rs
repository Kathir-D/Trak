//! `headless-spotify`'s CLI, read the way COMPAT asks for it (TODO 4.7).
//!
//! The sibling (`projects/headless-spotify`) hides Spotify from the Dock and
//! Cmd-Tab by setting `LSUIElement` in Spotify's `Info.plist`, and ships a CLI.
//! trak wants two things from it: a `headless` badge for the header, and a way
//! to start Spotify that does not steal focus — COMPAT rule 2 allows exactly
//! one launch, from the idle card, and names `headless-spotify launch` as the
//! preferred form of it.
//!
//! COMPAT asks for `status --json` "once at start and on demand". The sibling's
//! README ("`status --json` contract") pins the document to `schema` 1 with
//! twelve fields, of which trak reads three:
//!
//! - `headless` — `LSUIElement` is set or the Dock icon is gone. This is the
//!   badge, and it is a *configuration*, not a process: it can be true while
//!   Spotify is not running.
//! - `running` — a Spotify process is up. Only ever used to phrase a hint.
//! - `schema` — the contract's version, kept so "newer sibling" stays
//!   distinguishable from "no sibling".
//!
//! The other nine are read past. `installed` is the one that looks useful and
//! is not: it means Spotify.app exists, which trak learns from AppleScript every
//! poll and from its own idle card, and a field of that name next to trak's own
//! "is the tool here" would be a trap. `lsui_element` and `dock` are the two
//! halves of `headless`, so reading them separately would only let them
//! disagree. `ready`, `scriptable`, `player_state` and `backup_present` are
//! derived from what trak already knows or never asks, and a second opinion on
//! whether AppleScript answered is not one trak should take in place of its own.
//!
//! **Two rules shape the module.** Nothing here spawns a process: the commands
//! are *built* ([`status_command`], [`launch_command`]) and the caller runs them
//! on a worker, so nothing in the parse path can block the UI thread. And every
//! way of failing is a value rather than an error — no tool on `PATH`, a
//! `status` that exits 1 because Spotify is not ready, output trak cannot read,
//! a document from a sibling whose contract has moved on — all end at
//! [`Headless::unknown`] or a field that is `None`, which is no badge and no
//! hint. A sibling app is not worth a status line, let alone a panic.
//!
//! Unlike `sonar.rs`, an unknown `schema` is *read* rather than refused. Sonar
//! decides whether trak may write a volume, and being wrong there costs music;
//! everything here is a badge and a sentence, both of which have a safe
//! absence, so a new field is worth picking up and a renamed one degrades to no
//! badge on its own.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

/// The sibling's executable, and the only program name trak looks for.
pub const PROGRAM: &str = "headless-spotify";

/// Refuse output larger than this. A status document is a dozen short fields;
/// anything bigger is not one, and reading it is not worth the memory on the
/// thread that is about to draw a badge.
const MAX_BYTES: usize = 8 * 1024;

/// How deep the JSON may nest. A status object is flat, and the cap is so that
/// a broken or hostile document cannot drive the parser into a stack overflow.
const MAX_DEPTH: usize = 16;

/// Why a document is not a status. Both cases are things trak renders as
/// [`Headless::unknown`], never something that reaches the UI as a failure.
///
/// `thiserror` like every other error in trak, so the `Display` text is a line
/// somebody could read if a debug status ever shows it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// Not the JSON this module reads: a truncated document, trailing rubbish, or a
    /// value of a shape the reader cannot step over. A document that *is* readable
    /// JSON of some other shape is not an error — see [`parse`].
    #[error("headless-spotify: status output is not readable JSON")]
    Malformed,
    /// Larger than `MAX_BYTES`. Checked here as well as in the reader, because
    /// this is the only place a caller that did not come from a process's stdout
    /// goes through.
    #[error("headless-spotify: status output is {bytes} bytes")]
    TooLarge { bytes: usize },
}

/// What the sibling's `status --json` said, read before any of trak's rules.
///
/// Every field is optional, because every field is optional in the document: a
/// key trak does not find, finds twice, or finds as something else leaves this
/// exactly as it found it. [`Headless`] is the part trak decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Status {
    /// The `headless` field: Spotify has no Dock icon. `None` when the sibling
    /// did not say, which is the same as saying no as far as the badge is
    /// concerned.
    pub headless: Option<bool>,
    /// The `running` field, or `None`.
    pub running: Option<bool>,
    /// The `schema` field — the contract version, `1` in the sibling's README and
    /// absent from the copy installed here. Kept either way, so a sibling newer
    /// than this code stays distinguishable from an older one.
    pub schema: Option<u64>,
}

/// What trak believes about the sibling, and the only thing the TUI reads.
///
/// `installed` is the one field trak answers for itself: it is the PATH walk,
/// not the document. The document's own `installed` means Spotify.app exists,
/// which is a different question (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Headless {
    /// Whether `headless-spotify` is on `PATH`. COMPAT's "if `headless-spotify`
    /// is on `PATH`" is the gate for everything else here, and it is a fact
    /// about this machine rather than about any document.
    pub installed: bool,
    /// Spotify has no Dock icon — the sibling's `headless` field. `None` when
    /// nothing said, which draws no badge and no hint.
    pub hidden: Option<bool>,
    /// Whether Spotify is running, from the same document. Only ever used to
    /// phrase [`Headless::hint`].
    pub running: Option<bool>,
    /// The contract version the document carried.
    pub schema: Option<u64>,
}

impl Headless {
    /// Nothing believed: no tool on `PATH`, and nothing read.
    ///
    /// The answer to a missing tool, output that is not JSON, a document with
    /// none of the fields trak reads, and a caller's own "not installed".
    pub fn unknown() -> Self {
        Headless {
            installed: false,
            hidden: None,
            running: None,
            schema: None,
        }
    }

    /// What trak believes, from the PATH answer and whatever the sibling's
    /// status document said.
    ///
    /// Never fails and never panics. `installed` is kept whatever the document
    /// says, because the PATH walk is a fact about this machine and a truncated
    /// document does not un-install anything — it only loses the badge.
    pub fn from_status(installed: bool, json: &[u8]) -> Self {
        match parse(json) {
            Ok(status) => Headless {
                installed,
                hidden: status.headless,
                running: status.running,
                schema: status.schema,
            },
            // The document said nothing trak could use. The tool is still either
            // installed or not, and that is the half that picks a launch command.
            Err(_) => Headless {
                installed,
                hidden: None,
                running: None,
                schema: None,
            },
        }
    }

    /// The header badge, or `None` when there is nothing to badge.
    ///
    /// Lower-case, like `sonar`'s, and only ever for a Spotify that is actually
    /// hidden: COMPAT asks for a `headless` badge, and a sibling that is
    /// installed but switched off has not hidden anything.
    pub fn badge(&self) -> Option<&'static str> {
        match self.hidden {
            Some(true) => Some("headless"),
            Some(false) | None => None,
        }
    }

    /// One line for a footer or a toast, or `None` when there is nothing to say.
    ///
    /// COMPAT's reason for the whole integration is on this line: "if Spotify
    /// has no window / Dock icon, trak is the only display". So the hint names
    /// the command that brings Spotify's own window back, which is the one thing
    /// a hidden Spotify cannot do for the user.
    ///
    /// Unlike [`Headless::badge`], this waits for the tool to be installed: two of
    /// the three lines are about it, and advice naming a command the user cannot
    /// run is worse than no line.
    pub fn hint(&self) -> Option<String> {
        // No tool, nothing to explain: the user cannot run `headless-spotify`,
        // so a line mentioning it would be advice to install something trak does
        // not depend on.
        if !self.installed {
            return None;
        }
        match self.hidden {
            // Not running *and* hidden is the only combination where the user is
            // stuck: there is no Dock icon to click and nothing to click it on.
            // COMPAT rule 2 makes the idle card's `enter` the one launch trak ever
            // performs, and with the tool present that launch is the headless one.
            Some(true) if self.running == Some(false) => Some(
                "headless: Spotify is hidden and not running, press enter to launch it".to_string(),
            ),
            // Running, or running as far as anybody knows: the window is the way
            // back to the app trak is only partly showing.
            Some(true) => Some(
                "headless: Spotify has no Dock icon, open -a Spotify to show its window"
                    .to_string(),
            ),
            // The badge is absent, and this is why. Worth a line because the
            // sibling's presence is otherwise invisible.
            Some(false) => Some(
                "headless: headless-spotify is installed, Spotify still has a Dock icon"
                    .to_string(),
            ),
            None => None,
        }
    }
}

/// Whether `headless-spotify` is on this process's `PATH`.
///
/// COMPAT's gate for the badge, the hint and the launch command, asked once. A
/// missing `PATH` is a missing tool, not a failure.
pub fn is_installed() -> bool {
    match env::var_os("PATH") {
        Some(path) => is_installed_in(&path),
        None => false,
    }
}

/// [`is_installed`] against a `PATH` value of the caller's choosing.
///
/// Takes the environment *value* rather than reading it, so every test can point
/// it at a temp directory and no test has to write to the environment (which
/// `cargo test` runs in parallel threads, so writing it is not safe).
pub fn is_installed_in(path: &OsStr) -> bool {
    runnable_on(path, PROGRAM)
}

/// The PATH walk behind [`is_installed`], with the program name left open.
///
/// Written out rather than shelling out to `which`: that is a script on some
/// installs and absent on others, this is eight lines, and COMPAT has no problem
/// with a program that starts no process. `PATH` is searched left to right, as a
/// shell would, and the first hit wins.
fn runnable_on(path: &OsStr, program: &str) -> bool {
    env::split_paths(path).any(|dir| runnable(&dir.join(program)))
}

/// Whether a `PATH` entry holds a program a shell would actually run.
///
/// It has to exist, be a file rather than a directory, and carry an execute bit.
/// `loop_.rs`'s `which` checks only `is_file`, and this one does not: a file
/// named like the tool that nobody can run is not the tool, and calling it
/// installed would send COMPAT rule 2's one launch down a path that cannot work.
fn runnable(path: &Path) -> bool {
    // `metadata` and not `symlink_metadata`: a symlink into a Homebrew prefix is
    // the normal way this tool is installed, and the permissions that matter are
    // the target's.
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    meta.is_file() && meta.permissions().mode() & 0o111 != 0
}

/// The command that reads the sibling's status: `headless-spotify status --json`.
///
/// Built, not run: the caller spawns it on a worker thread and reads stdout,
/// because COMPAT asks for this once at start and on demand and the UI thread
/// must not wait on a process that reads a plist and asks AppleScript.
///
/// The sibling's exit code is 0 only when it is ready, so a caller must read
/// stdout even on exit 1 — which is why this returns a command and not a
/// `Headless`.
pub fn status_command() -> Command {
    let mut cmd = Command::new(PROGRAM);
    cmd.args(["status", "--json"]);
    cmd
}

/// The one launch trak ever performs, as a command (COMPAT rule 2).
///
/// The sibling's `launch` is preferred because it is its supported way to start
/// Spotify in the background, and it is a no-op when Spotify is already running.
pub fn launch_command() -> Command {
    launch_command_for(is_installed())
}

/// [`launch_command`] with the "is it installed" answer supplied.
///
/// Building it is the whole job: COMPAT rule 2 is about the *arguments*, so this
/// never spawns anything and the exact argv of both branches is asserted in the
/// tests below.
pub fn launch_command_for(installed: bool) -> Command {
    if installed {
        let mut cmd = Command::new(PROGRAM);
        cmd.arg("launch");
        cmd
    } else {
        let mut cmd = Command::new("open");
        // `-g` is "do not bring the app to the front" and `-j` is "do not hide
        // the other running apps". Together they are COMPAT rule 2's "background,
        // no focus steal, no Dock bounce". A bare `open -a Spotify` would focus
        // Spotify and bounce the Dock out of a TUI, so neither flag may be
        // dropped, and the `-a Spotify` is how trak names the app without
        // hardcoding a bundle path (rule 1).
        cmd.args(["-g", "-j", "-a", "Spotify"]);
        cmd
    }
}

/// Read one `status --json` document and report the three fields trak uses.
///
/// The shape is the only thing judged here, and no version is: a document trak
/// does not recognise still parses, and it is [`Headless::from_status`] that
/// decides what to do with what came back. Splitting the two is what lets this
/// be tested against every shape trak has never seen without a `Headless` in the
/// way.
///
/// Every field is optional, so nothing here can fail for want of one. A document
/// that is valid JSON of some other shape entirely — an array, a bare number, a
/// future sibling printing something else — comes back as
/// [`Status::default`], because a companion app printing something trak cannot
/// read is not a fact about Spotify and must not raise anything. Only a document
/// that is not JSON at all, or is larger than `MAX_BYTES`, is refused.
pub fn parse(bytes: &[u8]) -> Result<Status, ParseError> {
    if bytes.len() > MAX_BYTES {
        return Err(ParseError::TooLarge { bytes: bytes.len() });
    }
    let json = parse_json(bytes)?;
    Ok(Status {
        headless: json.flag("headless"),
        running: json.flag("running"),
        schema: json.integer("schema"),
    })
}

/// Just enough JSON for one flat object of flags and a version number.
///
/// The same shape, and the same strictness, as the reader in `sonar.rs` and
/// `lyrics.rs`, for the same reason: the crate has no serde in it, and a
/// dependency for one object of two booleans and an integer is not worth it.
/// The document has to be exactly one value, a number has to be a number, and a
/// truncated or over-deep body is refused rather than half-read.
///
/// Every value kind is here, not only the three trak reads, so that an unknown
/// field of any shape is skipped rather than refused — the sibling's rules are
/// "new fields may be added under `schema` 1", so a key trak has never heard of
/// is the *expected* case, not an error.
#[derive(Debug)]
enum Json {
    Null,
    Bool(bool),
    /// The token as it was written, once it has been checked to be a number. Text
    /// rather than an `f64` so that a schema version is read as the integer it
    /// has to be instead of as a float to be rounded.
    Num(String),
    /// Parsed and kept, never read: a status object has no string trak reads, and
    /// one that grows one must not become a document trak cannot step over.
    #[allow(
        dead_code,
        reason = "parsed so an unknown field of this shape is skipped"
    )]
    Str(String),
    #[allow(
        dead_code,
        reason = "parsed so an unknown field of this shape is skipped"
    )]
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        let Json::Obj(fields) = self else { return None };
        fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// A boolean field, or `None` for a key that is absent, repeated out of
    /// disagreement, or written as something that is not a boolean.
    ///
    /// `None` rather than `false`, unlike `lyrics.rs`'s `flag`: absence and "no"
    /// are different claims about the sibling, and only the second one is a fact
    /// trak can put a badge on.
    fn flag(&self, key: &str) -> Option<bool> {
        match self.get(key) {
            Some(Json::Bool(value)) => Some(*value),
            _ => None,
        }
    }

    /// A field as a whole number. `None` for a missing field, a `null`, a
    /// string, and a number that is not an integer: `1.0` and `2e0` are things
    /// the sibling does not write, and rounding either to the integer beside it
    /// would be a guess about which contract is in force.
    fn integer(&self, key: &str) -> Option<u64> {
        match self.get(key) {
            Some(Json::Num(text)) => text.parse().ok(),
            _ => None,
        }
    }
}

fn parse_json(bytes: &[u8]) -> Result<Json, ParseError> {
    let mut p = JsonParser {
        bytes,
        at: 0,
        depth: 0,
    };
    p.skip_space();
    let value = p.value()?;
    p.skip_space();
    // Anything after the document means trak is not reading what it thinks it is.
    if p.at != p.bytes.len() {
        return Err(ParseError::Malformed);
    }
    Ok(value)
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    at: usize,
    depth: usize,
}

impl JsonParser<'_> {
    fn value(&mut self) -> Result<Json, ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(ParseError::Malformed);
        }
        let out = match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') => self.literal(b"true", Json::Bool(true)),
            Some(b'f') => self.literal(b"false", Json::Bool(false)),
            Some(b'n') => self.literal(b"null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(ParseError::Malformed),
        };
        self.depth -= 1;
        out
    }

    fn object(&mut self) -> Result<Json, ParseError> {
        self.expect(b'{')?;
        let mut fields = Vec::new();
        self.skip_space();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Json::Obj(fields));
        }
        loop {
            self.skip_space();
            let key = self.string()?;
            self.skip_space();
            self.expect(b':')?;
            self.skip_space();
            let value = self.value()?;
            fields.push((key, value));
            self.skip_space();
            match self.next() {
                Some(b',') => {}
                Some(b'}') => return Ok(Json::Obj(fields)),
                _ => return Err(ParseError::Malformed),
            }
        }
    }

    fn array(&mut self) -> Result<Json, ParseError> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_space();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.skip_space();
            items.push(self.value()?);
            self.skip_space();
            match self.next() {
                Some(b',') => {}
                Some(b']') => return Ok(Json::Arr(items)),
                _ => return Err(ParseError::Malformed),
            }
        }
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            match self.next().ok_or(ParseError::Malformed)? {
                b'"' => break,
                b'\\' => {
                    let escape = self.next().ok_or(ParseError::Malformed)?;
                    self.escape(escape, &mut out)?;
                }
                byte => out.push(byte),
            }
        }
        // The bytes came from a process and only escapes were added, so this can
        // only fail on a document that is not UTF-8, which the branches below do
        // not rule out. It is checked anyway, because a panic here is never
        // acceptable.
        String::from_utf8(out).map_err(|_| ParseError::Malformed)
    }

    fn escape(&mut self, escape: u8, out: &mut Vec<u8>) -> Result<(), ParseError> {
        let ch = match escape {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            // The sibling's own strings are ASCII, but a document from a future
            // headless-spotify may carry anything, and the same rule as
            // `sonar.rs` applies: a character outside the basic plane is two
            // escapes, and only a well-formed pair is one character.
            b'u' => {
                let unit = self.hex4()?;
                let code = if (0xD800..0xDC00).contains(&unit) {
                    let low = self.paired_unit()?;
                    // This pairing is what turns two escapes into one character.
                    0x1_0000 + ((u32::from(unit) - 0xD800) << 10) + (u32::from(low) - 0xDC00)
                } else {
                    u32::from(unit)
                };
                // A low surrogate on its own, or an unpaired half, has no
                // character behind it, and guessing one is how a key turns into
                // replacement characters.
                char::from_u32(code).ok_or(ParseError::Malformed)?
            }
            _ => return Err(ParseError::Malformed),
        };
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        Ok(())
    }

    /// The second half of a surrogate pair, including the `\u` that must sit
    /// between the halves.
    fn paired_unit(&mut self) -> Result<u16, ParseError> {
        self.expect(b'\\')?;
        self.expect(b'u')?;
        let low = self.hex4()?;
        if !(0xDC00..0xE000).contains(&low) {
            return Err(ParseError::Malformed);
        }
        Ok(low)
    }

    fn number(&mut self) -> Result<Json, ParseError> {
        let start = self.at;
        while matches!(
            self.peek(),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.at += 1;
        }
        // The slice is the ASCII the scan just walked over.
        let text =
            std::str::from_utf8(&self.bytes[start..self.at]).map_err(|_| ParseError::Malformed)?;
        // Checked and kept: a token that is not a number is a broken document,
        // and skipping it without checking would let it through as one whose
        // fields are merely absent.
        text.parse::<f64>()
            .map_err(|_| ParseError::Malformed)
            .map(|_| Json::Num(text.to_string()))
    }

    fn literal(&mut self, word: &[u8], value: Json) -> Result<Json, ParseError> {
        if self.bytes[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(ParseError::Malformed)
        }
    }

    fn hex4(&mut self) -> Result<u16, ParseError> {
        let mut value: u16 = 0;
        for _ in 0..4 {
            let digit = self.next().ok_or(ParseError::Malformed)?;
            let nibble = match digit {
                b'0'..=b'9' => digit - b'0',
                b'a'..=b'f' => digit - b'a' + 10,
                b'A'..=b'F' => digit - b'A' + 10,
                _ => return Err(ParseError::Malformed),
            };
            value = value * 16 + u16::from(nibble);
        }
        Ok(value)
    }

    fn expect(&mut self, byte: u8) -> Result<(), ParseError> {
        if self.next() == Some(byte) {
            Ok(())
        } else {
            Err(ParseError::Malformed)
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        Some(byte)
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The sibling's README example, verbatim and in the key order its
    /// `JSONSerialization` actually emits (sorted keys), with a normal Spotify.
    const NORMAL: &str = concat!(
        r#"{"app":"/Applications/Spotify.app","backup_present":false,"#,
        r#""bundle":"com.spotify.client","dock":"visible","headless":false,"#,
        r#""installed":true,"lsui_element":null,"player_state":"playing","#,
        r#""ready":false,"running":true,"schema":1,"scriptable":true}"#
    );

    /// The same document with the tool doing its job: no Dock icon, and the
    /// backup a `hide` leaves behind.
    const HIDDEN: &str = concat!(
        r#"{"app":"/Applications/Spotify.app","backup_present":true,"#,
        r#""bundle":"com.spotify.client","dock":"hidden","headless":true,"#,
        r#""installed":true,"lsui_element":true,"player_state":"playing","#,
        r#""ready":true,"running":true,"schema":1,"scriptable":true}"#
    );

    /// The document the sibling *installed on the owner's machine* printed, captured
    /// 2026-09-30 from `headless-spotify status --json` (exit 1, because `ready` is
    /// false while Spotify keeps its Dock icon). Two things about it are worth
    /// pinning and neither is in the README's example: the forward slashes in the
    /// app path come back escaped, and it carries **no `schema` at all** — the
    /// versioned contract is newer than the install trak would find here. trak has
    /// to read the fields it knows out of either.
    const CAPTURED: &str = concat!(
        r#"{"app":"\/Applications\/Spotify.app","backup_present":false,"#,
        r#""bundle":"com.spotify.client","dock":"visible","headless":false,"#,
        r#""installed":true,"lsui_element":null,"player_state":"playing","#,
        r#""ready":false,"running":true,"scriptable":true}"#
    );

    fn parsed(json: &str) -> Status {
        parse(json.as_bytes()).unwrap_or_else(|e| panic!("{json:?} should parse: {e}"))
    }

    /// The argv of a built command, so a test can state it exactly.
    fn argv(cmd: &Command) -> (String, Vec<String>) {
        (
            cmd.get_program().to_string_lossy().into_owned(),
            cmd.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
        )
    }

    #[test]
    fn a_status_document_parses_with_every_field_trak_reads() {
        let status = parsed(NORMAL);
        assert_eq!(
            status,
            Status {
                headless: Some(false),
                running: Some(true),
                schema: Some(1),
            }
        );
        let hidden = parsed(HIDDEN);
        assert_eq!(hidden.headless, Some(true));
        assert_eq!(hidden.running, Some(true));
        assert_eq!(hidden.schema, Some(1));

        // And through the tolerant entry point, with the tool on `PATH`.
        let headless = Headless::from_status(true, HIDDEN.as_bytes());
        assert!(headless.installed);
        assert_eq!(headless.hidden, Some(true));
        assert_eq!(headless.running, Some(true));
        assert_eq!(headless.schema, Some(1));
        assert_eq!(headless.badge(), Some("headless"));
    }

    #[test]
    fn fields_trak_has_never_heard_of_are_ignored() {
        // The sibling's rule is that new fields arrive under `schema` 1, so a key
        // trak has no name for is the expected case and not a broken document.
        let json = concat!(
            r#"{"schema":1,"headless":true,"running":true,"#,
            r#""note":"written by a newer headless-spotify","dock":"hidden","#,
            r#""layers":[1,2,{"x":null}],"badge":{"text":"headless"},"#,
            r#""headless":null,"running":null,"running":true,"schema":1}"#,
            "\n"
        );
        let status = parsed(json);
        assert_eq!(
            status,
            Status {
                headless: Some(true),
                running: Some(true),
                schema: Some(1),
            },
            "the first of a repeated key wins"
        );
        // And the unknown keys with nothing trak reads repeated.
        let json = concat!(
            r#"{"schema":1,"headless":true,"running":true,"#,
            r#""future":{"a":[1,2,3]},"what":"\u00e9","n":null}"#
        );
        let status = parsed(json);
        assert_eq!(status.headless, Some(true));
        assert_eq!(status.running, Some(true));
        assert_eq!(status.schema, Some(1));
    }

    #[test]
    fn a_field_that_is_missing_or_the_wrong_type_is_simply_unknown() {
        // `{}` is a document trak cannot learn anything from, and it is not an
        // error: the sibling is installed and has said nothing.
        assert_eq!(parsed("{}"), Status::default());
        let headless = Headless::from_status(true, b"{}");
        assert!(headless.installed, "the PATH walk still stands");
        assert_eq!(headless.badge(), None);
        assert_eq!(headless.hint(), None);

        // Every shape that is not the one trak reads leaves that field unknown
        // rather than making the whole document unreadable. Absent and "not a
        // boolean" have to land in the same place, or a sibling that retypes a
        // field would quietly start drawing the wrong badge.
        let not_a_bool = [r#"null"#, r#"1"#, r#""true""#, r#"[]"#, r#"{"yes":true}"#];
        for value in not_a_bool {
            let json = format!(r#"{{"schema":1,"headless":{value},"running":{value}}}"#);
            let status = parsed(&json);
            assert_eq!(status.headless, None, "headless {value}");
            assert_eq!(status.running, None, "running {value}");
            assert_eq!(status.schema, Some(1), "the rest still reads: {value}");
        }
        // A schema that is not a whole number, including one that would parse as a
        // float: rounding `1.0` to 1 would be a guess about which contract is in
        // force.
        for value in [
            r#"null"#, r#"true"#, r#"1.0"#, r#"2e0"#, r#""1""#, r#""1 ""#, r#"-1"#, r#"[1]"#,
            r#"{}"#,
        ] {
            let json = format!(r#"{{"schema":{value},"headless":true}}"#);
            let status = parsed(&json);
            assert_eq!(status.schema, None, "schema {value}");
            assert_eq!(status.headless, Some(true), "the rest still reads: {value}");
        }
        // And a version far beyond a `u64` is not a version either.
        let json = format!(r#"{{"schema":{},"headless":true}}"#, "9".repeat(40));
        assert_eq!(parsed(&json).schema, None);

        // And a document trak cannot read at all keeps the PATH answer and loses
        // everything else, rather than stopping anything.
        for json in [
            b"not json".as_slice(),
            b"".as_slice(),
            b"{".as_slice(),
            b"[]".as_slice(),
            b"{\"headless\":true} trailing".as_slice(),
        ] {
            let headless = Headless::from_status(true, json);
            assert_eq!(
                headless,
                Headless {
                    installed: true,
                    ..Headless::unknown()
                },
                "{json:?}"
            );
            assert_eq!(headless.badge(), None);
            assert_eq!(headless.hint(), None);
        }
    }

    #[test]
    fn a_future_schema_is_read_rather_than_refused() {
        // The opposite of `sonar.rs`'s rule, and deliberately: a sibling whose
        // contract has moved on may still be hiding Spotify perfectly well, and
        // everything trak decides here has a safe absence.
        for schema in [0, 2, 3, 99, u64::MAX] {
            let json = format!(r#"{{"schema":{schema},"headless":true,"running":true}}"#);
            let status = parsed(&json);
            assert_eq!(status.schema, Some(schema));
            assert_eq!(status.headless, Some(true), "schema {schema}");
            let headless = Headless::from_status(true, json.as_bytes());
            assert_eq!(headless.badge(), Some("headless"), "schema {schema}");
        }
        // And a document with no `schema` at all: the contract's fields are still
        // read, because they are the ones trak was written against.
        let headless = Headless::from_status(true, br#"{"headless":true}"#);
        assert_eq!(headless.schema, None);
        assert_eq!(headless.badge(), Some("headless"));
        // A version with a leading zero is still a version. That leniency belongs
        // to the reader `sonar.rs` and `lyrics.rs` share, and reading the document
        // beats refusing all of it over one odd token.
        assert_eq!(parsed(r#"{"schema":01,"headless":true}"#).schema, Some(1));
    }

    #[test]
    fn the_document_the_installed_sibling_prints_is_read() {
        // The real one, not the README's: escaped slashes, no `schema`.
        let status = parsed(CAPTURED);
        assert_eq!(
            status,
            Status {
                headless: Some(false),
                running: Some(true),
                schema: None
            },
            "a missing `schema` is not a reason to refuse the document"
        );
        let headless = Headless::from_status(true, CAPTURED.as_bytes());
        assert!(headless.installed);
        assert_eq!(headless.badge(), None, "Spotify still has a Dock icon");
        assert!(
            headless.hint().is_some(),
            "and that is the one thing worth saying about it"
        );
    }

    #[test]
    fn rubbish_is_malformed_rather_than_a_panic() {
        for body in [
            "{",
            "}",
            "",
            "   ",
            "{,}",
            r#"{"headless":true,}"#,
            r#"{"headless" true}"#,
            r#"{"headless":}"#,
            r#"{"headless":tru}"#,
            r#"{"headless":truex}"#,
            r#"{"headless":truetrue}"#,
            r#"{headless:true}"#,
            r#"{"headless":"unterminated}"#,
            r#"{"headless":"bad\qescape"}"#,
            r#"{"note":"\q"}"#,
            r#"{"headless":true} {"running":true}"#,
            "not json at all",
            r#"{"schema":1.2.3,"headless":true}"#,
            // A byte-order mark, which a writer is free to add and this reader is
            // not: refused rather than skipped, because skipping it would mean
            // guessing where a document starts.
            "\u{feff}{\"headless\":true}",
        ] {
            assert!(
                matches!(parse(body.as_bytes()), Err(ParseError::Malformed)),
                "{body:?} should be malformed"
            );
        }
    }

    #[test]
    fn a_document_that_is_not_an_object_is_nothing_known_rather_than_a_failure() {
        // A sibling that prints something else entirely — a different shape under a
        // future `schema`, or help text because a flag changed name — is not
        // something trak should raise about. It has no fields trak reads, so it
        // reads as nothing known, and the badge simply stays off.
        for body in [
            "[]",
            "[{\"headless\":true}]",
            "null",
            "true",
            "1",
            r#""hidden""#,
            r#"["a","b"]"#,
            r#"{"note":"no fields trak reads"}"#,
        ] {
            assert_eq!(
                parse(body.as_bytes()),
                Ok(Status::default()),
                "{body:?} should be nothing known"
            );
        }
    }

    #[test]
    fn a_truncated_status_document_is_malformed_at_every_cut() {
        // The sibling prints its whole object in one write, so trak should never
        // see half of one — but "should never" is not a reason to have an
        // unchecked index.
        for cut in 0..HIDDEN.len() {
            let body = &HIDDEN.as_bytes()[..cut];
            assert!(
                matches!(parse(body), Err(ParseError::Malformed)),
                "cut at {cut} ({:?}) should be malformed",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn a_status_document_that_is_not_text_is_malformed() {
        for body in [
            &b"{\"headless\":\"\xff\xfe\"}"[..],
            &b"{\"headless\":true}\xff"[..],
            &b"\xff\xfe\xfd"[..],
            // A lone high surrogate, and a high one with nothing after it.
            &br#"{"note":"\uD83C"}"#[..],
            &br#"{"note":"\uD83C"}[..],
            &br#"{"note":"\uD83C\u0041"}"#[..],
        ] {
            assert!(
                matches!(parse(body), Err(ParseError::Malformed)),
                "{:?} should be malformed",
                String::from_utf8_lossy(body)
            );
        }
        // Escapes are still decoded, so a key or a future string written with them
        // is stepped over rather than refused as broken.
        assert_eq!(parsed(r#"{"\u0068eadless":true}"#).headless, Some(true));
    }

    #[test]
    fn a_deep_body_is_refused_instead_of_overflowing_the_stack() {
        for body in [
            "[".repeat(2000),
            format!(r#"{{"headless":true,"x":{}}}"#, "[".repeat(2000)),
        ] {
            assert!(
                matches!(parse(body.as_bytes()), Err(ParseError::Malformed)),
                "a deep body should be refused"
            );
        }
    }

    #[test]
    fn a_document_larger_than_the_cap_is_refused() {
        // A status object is a dozen short fields. Anything bigger is not one,
        // and the caller should not pay memory to find out what it is.
        let padding = "x".repeat(MAX_BYTES);
        let body = format!(r#"{{"note":"{padding}","headless":true}}"#);
        assert!(matches!(
            parse(body.as_bytes()),
            Err(ParseError::TooLarge { .. })
        ));
        // And one that is only just under it still parses.
        let short = format!(
            r#"{{"note":"{}","headless":true}}"#,
            "x".repeat(MAX_BYTES / 2)
        );
        assert_eq!(parsed(&short).headless, Some(true));
    }

    #[test]
    fn a_tool_on_the_path_is_found_and_one_that_is_not_is_refused() {
        let dir = TempDir::new("installed");
        let program = dir.path().join(PROGRAM);
        fs::write(&program, b"#!/bin/sh\nexit 0\n").expect("write the program");
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).expect("make it runnable");

        // The document says nothing, and the tool is still found: the PATH walk
        // does not need the sibling's cooperation.
        assert!(is_installed_in(dir.path().as_os_str()));
        // A name that cannot be on any PATH is refused.
        assert!(!runnable_on(
            dir.path().as_os_str(),
            "definitely-not-a-real-program-xyz"
        ));
        // A file nobody can run is not the tool, which a bare `is_file` would call
        // it.
        fs::set_permissions(&program, fs::Permissions::from_mode(0o644))
            .expect("strip the execute bit");
        assert!(!is_installed_in(dir.path().as_os_str()), "not executable");
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).expect("put it back");

        // A directory of the same name is not the tool either.
        let empty = TempDir::new("empty");
        assert!(!is_installed_in(empty.path().as_os_str()));
        fs::create_dir(empty.path().join(PROGRAM)).expect("make the shadowing directory");
        assert!(!is_installed_in(empty.path().as_os_str()), "a directory");

        // Later entries in `PATH` are searched, and a `PATH` with nothing in it has
        // nothing to find. No test here *writes* `PATH`: `cargo test` shares one
        // environment across its threads, so setting it would be a race.
        let joined = env::join_paths([empty.path(), dir.path()]).expect("join two paths");
        assert!(is_installed_in(&joined), "the second entry");
        assert!(!is_installed_in(OsStr::new("")));
    }

    #[test]
    fn launch_command_matches_the_real_path_of_this_machine() {
        // Reading `PATH` is safe from any thread; this test never writes it. The
        // owner's machine may have the sibling installed or not, and both answers
        // are legal — but the command has to be the matching branch, which is the
        // part `loop_.rs` could not check without string-matching a `Debug`.
        let expected = if is_installed() {
            (PROGRAM.to_string(), vec!["launch".to_string()])
        } else {
            (
                "open".to_string(),
                ["-g", "-j", "-a", "Spotify"]
                    .iter()
                    .map(|a| a.to_string())
                    .collect(),
            )
        };
        assert_eq!(argv(&launch_command()), expected);
    }

    #[test]
    fn the_real_path_has_a_program_and_no_invented_one() {
        // The walk itself, against the environment it will actually be used with:
        // a program macOS always has, and a name nothing has.
        let Some(path) = env::var_os("PATH") else {
            return;
        };
        assert!(
            runnable_on(&path, "sh"),
            "sh is on PATH on every macOS with a shell"
        );
        assert!(!runnable_on(&path, "definitely-not-a-real-program-xyz"));
    }

    /// COMPAT rule 2 is about these arguments and nothing else, so both branches
    /// are stated exactly: a missing `-g` focuses Spotify, a missing `-j` bounces
    /// the Dock out from under a TUI.
    #[test]
    fn the_launch_command_is_exactly_the_sibling_when_it_is_installed() {
        assert_eq!(
            argv(&launch_command_for(true)),
            ("headless-spotify".to_string(), vec!["launch".to_string()],)
        );
    }

    #[test]
    fn the_launch_command_is_exactly_open_when_the_sibling_is_absent() {
        assert_eq!(
            argv(&launch_command_for(false)),
            (
                "open".to_string(),
                ["-g", "-j", "-a", "Spotify"]
                    .iter()
                    .map(|a| a.to_string())
                    .collect(),
            )
        );
    }

    #[test]
    fn the_status_command_asks_for_the_versioned_document() {
        assert_eq!(
            argv(&status_command()),
            (
                "headless-spotify".to_string(),
                ["status", "--json"].iter().map(|a| a.to_string()).collect(),
            )
        );
    }

    #[test]
    fn the_badge_is_shown_only_for_a_spotify_that_is_actually_hidden() {
        for (hidden, badge) in [
            (Some(true), Some("headless")),
            (Some(false), None),
            (None, None),
        ] {
            let headless = Headless {
                installed: true,
                hidden,
                running: Some(true),
                schema: Some(1),
            };
            assert_eq!(headless.badge(), badge, "{hidden:?}");
        }
        // The badge is a fact about the Dock, not about `PATH`: it follows the document
        // alone, so a caller that has a status document can draw it whatever it
        // answered about the tool. `installed` decides the launch command and
        // whether a hint is worth a line, and nothing else.
        assert_eq!(
            Headless {
                installed: false,
                hidden: Some(true),
                running: Some(true),
                schema: Some(1)
            }
            .badge(),
            Some("headless")
        );
    }

    #[test]
    fn the_hint_says_how_to_get_spotifys_window_back() {
        let at = |hidden, running| Headless {
            installed: true,
            hidden,
            running,
            schema: Some(1),
        };
        // Hidden and not running is the stuck case, and it is the idle card's
        // `enter` that gets out of it.
        assert_eq!(
            at(Some(true), Some(false)).hint(),
            Some(
                "headless: Spotify is hidden and not running, press enter to launch it".to_string()
            )
        );
        // Hidden: name the command, because trak is now the only display.
        assert_eq!(
            at(Some(true), Some(true)).hint(),
            Some(
                "headless: Spotify has no Dock icon, open -a Spotify to show its window"
                    .to_string()
            )
        );
        // Hidden as far as anybody knows: the same line, because the difference is
        // not worth a second sentence.
        assert_eq!(
            at(Some(true), None).hint(),
            at(Some(true), Some(true)).hint()
        );
        // Installed but switched off explains the missing badge.
        assert_eq!(
            at(Some(false), Some(true)).hint(),
            Some(
                "headless: headless-spotify is installed, Spotify still has a Dock icon"
                    .to_string()
            )
        );
        // A tool that is not installed has nothing to say, whatever a document
        // claims: a footer that tells the user to run `open -a Spotify` is fine,
        // one that tells them to run a command they do not have is not.
        assert_eq!(at(None, Some(true)).hint(), None);
        assert_eq!(at(None, None).hint(), None);
        assert_eq!(Headless::unknown().hint(), None);
        assert_eq!(
            Headless {
                installed: false,
                ..at(Some(true), Some(true))
            }
            .hint(),
            None
        );
    }

    #[test]
    fn every_hint_and_badge_is_one_line() {
        // A hint goes in a status line, so a second line would eat the footer keys
        // (TODO 4.8). Every combination of the three facts is checked, including
        // the ones no document produced.
        for installed in [false, true] {
            for hidden in [None, Some(false), Some(true)] {
                for running in [None, Some(false), Some(true)] {
                    for schema in [None, Some(1)] {
                        let headless = Headless {
                            installed,
                            hidden,
                            running,
                            schema,
                        };
                        if let Some(hint) = headless.hint() {
                            assert!(!hint.is_empty(), "{headless:?}");
                            assert!(!hint.contains('\n'), "one line: {hint:?}");
                            assert!(!hint.contains('\r'), "one line: {hint:?}");
                            assert!(!hint.contains("panicked"), "{hint:?}");
                            assert!(hint.starts_with("headless: "), "{hint:?}");
                        }
                        if let Some(badge) = headless.badge() {
                            assert_eq!(badge, "headless");
                            assert!(!badge.contains('\n'), "one line: {badge:?}");
                        }
                    }
                }
            }
        }
        // The errors are only ever shown in a debug status, and are one line too.
        for e in [
            ParseError::Malformed,
            ParseError::TooLarge {
                bytes: MAX_BYTES + 1,
            },
        ] {
            let line = e.to_string();
            assert!(!line.is_empty());
            assert!(!line.contains('\n'), "one line: {line:?}");
        }
    }

    #[test]
    fn unknown_is_the_answer_when_there_is_nothing_to_believe() {
        let unknown = Headless::unknown();
        assert!(!unknown.installed);
        assert_eq!(unknown.hidden, None);
        assert_eq!(unknown.running, None);
        assert_eq!(unknown.schema, None);
        assert_eq!(unknown.badge(), None);
        assert_eq!(unknown.hint(), None);
        // A document is read even when the tool is not installed, so a caller that has
        // already asked `is_installed` can hand both answers over at once; the
        // badge is still drawn, because the tool on someone else's `PATH` is not
        // the thing that hides Spotify.
        assert_eq!(
            Headless::from_status(false, HIDDEN.as_bytes()),
            Headless {
                installed: false,
                hidden: Some(true),
                running: Some(true),
                schema: Some(1)
            }
        );
        // A document with nothing trak reads, and one it cannot read at all, are
        // the same answer: no fields, and `installed` untouched.
        assert_eq!(
            Headless::from_status(false, NORMAL.as_bytes()),
            Headless {
                installed: false,
                hidden: Some(false),
                running: Some(true),
                schema: Some(1)
            }
        );
        assert_eq!(
            Headless::from_status(false, b"{}"),
            Headless::unknown(),
            "a document with none of trak's fields"
        );
        assert_eq!(
            Headless::from_status(false, b"{oops"),
            Headless::unknown(),
            "a document that is not JSON"
        );
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "trak-headless-test-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
