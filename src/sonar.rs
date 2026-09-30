//! Sonar's state file, read in a way that cannot stop anything (TODO 4.6).
//!
//! Sonar is a menu-bar item for skip / previous and auto-pause (`projects/Sonar`).
//! It publishes what its engine is doing to
//! `~/Library/Application Support/Sonar/state.json`, pinned by
//! `docs/AGENT-PROMPTS.md` prompt 1 and read by `docs/COMPAT.md` under
//! "Sonar state file":
//!
//! ```json
//! {"v":1,"state":"idle|ducking|ducked|resuming","pid":501,"since":1790000000}
//! ```
//!
//! The four fields, and why each one is here:
//!
//! - `v` — the schema version. Only [`KNOWN_VERSION`] is believed. A file from a
//!   newer Sonar is refused rather than read as version 1, because `state` is
//!   free to have grown meanings trak would misread.
//! - `state` — the engine's phase. The three duck phases are what COMPAT rule 3
//!   is about: trak must not write a volume during one, and `m` is disabled.
//! - `pid` — **Spotify's** pid, or `null`. Not Sonar's. It says which player the
//!   duck is about, so a file left behind by a Spotify that has since quit is not
//!   applied to the one trak is now showing.
//! - `since` — Unix seconds, when the phase began. Cosmetic: it is what a notice
//!   counts up from, and its absence does not make a state untrusted.
//!
//! The rule that shapes the whole module: **every way of failing is a value, not
//! an error.** No file, an unreadable file, bad JSON, a version trak does not
//! know, a pid that is not this Spotify, and a file too old to trust all end at
//! [`SonarState::unknown`], which renders as plain "paused" and a missing badge.
//! COMPAT asks for that silence, and a sibling app is not worth a status line —
//! let alone a panic — when its agent has not written the writer yet.
//!
//! Everything that decides anything is pure: [`parse`] takes bytes, [`is_fresh`]
//! and [`resolve`] take times and a [`Trust`], and [`SonarState::notice_at`]
//! takes the clock as an argument. Only [`read_state`] and its two helpers touch
//! the filesystem, so the policy is tested without a file, a clock, or a running
//! Sonar.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// The only `v` trak reads. COMPAT's example and AGENT-PROMPTS prompt 1 both pin
/// the schema at 1, and trak is the only reader that matters: reading a newer
/// file as an older one is how a duck gets obeyed that is not a duck.
pub const KNOWN_VERSION: u64 = 1;

/// How long a state file stays believable on its own: ten minutes, from COMPAT's
/// "the file is fresh (mtime within 10 minutes, or Sonar's process is alive)".
/// The other half of that rule is [`Trust::Checked`]'s `sonar_alive`, which needs
/// a process list and so cannot be answered from a file.
pub const FRESH_FOR: Duration = Duration::from_secs(10 * 60);

/// Refuse a file larger than this. A state file is four fields; anything bigger
/// is not one, and a 500 ms poll should not be a way to spend the owner's memory.
const MAX_BYTES: u64 = 4 * 1024;

/// How deep the JSON may nest. Sonar's object is flat, and the cap is so that a
/// broken or hostile file cannot drive the parser into a stack overflow on the
/// thread doing the polling.
const MAX_DEPTH: usize = 16;

/// Why a file is not a state file. Both cases are states trak renders
/// ([`SonarState::unknown`]), never something that reaches the UI as a failure.
///
/// `thiserror` like every other error in trak, so the `Display` text is a line
/// somebody could read if a debug status ever shows it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// Not the JSON this module reads: not an object, a field of the wrong type,
    /// a truncated document, trailing rubbish, a `state` with no matching word,
    /// or a `v` that is not a number.
    #[error("sonar: state file is not readable JSON")]
    Malformed,
    /// Larger than [`MAX_BYTES`]. Checked here as well as in the reader, because
    /// this is the last place that can say no and the only one a caller who did
    /// not come through [`read_state`] goes through.
    #[error("sonar: state file is {bytes} bytes")]
    TooLarge { bytes: u64 },
}

/// What Sonar says it is doing, in the four words its file uses.
///
/// [`SonarPhase::Unknown`] is not one of them. It is what trak has when nothing
/// was believed, and keeping it here rather than as a separate `Option` is what
/// stops a caller from treating "no Sonar" as a phase to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SonarPhase {
    /// Nothing trak can act on. See [`SonarState::unknown`].
    Unknown,
    /// Sonar is not ducking. It also writes this on quit and when Auto-Pause is
    /// switched off, so it means "Sonar is not holding your music" and not
    /// "Sonar is here".
    Idle,
    /// Fading the volume down on the way to pausing.
    Ducking,
    /// Paused with the volume down: the value trak must not write back.
    Ducked,
    /// Fading the volume back up on the way to playing. The reading is still
    /// mid-fade, so a write here is a write into a fade.
    Resuming,
}

impl SonarPhase {
    /// The three phases in which Sonar owns the volume. COMPAT rule 3: trak
    /// writes no volume across all of them and `m` is disabled, because a fade is
    /// under way in all of them, not only in the paused one.
    pub fn is_duck(self) -> bool {
        matches!(
            self,
            SonarPhase::Ducking | SonarPhase::Ducked | SonarPhase::Resuming
        )
    }
}

/// One state file, read and not yet believed.
///
/// The fields are what the file said, not what trak made of it: the version is
/// kept even when it is not [`KNOWN_VERSION`], so "Sonar is newer" and "there is
/// no Sonar" stay distinguishable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SonarReport {
    /// The `v` field.
    pub version: u64,
    /// The `state` field. Never [`SonarPhase::Unknown`]: a state with no word is
    /// [`ParseError::Malformed`], not a fifth phase.
    pub phase: SonarPhase,
    /// The `pid` field — the Spotify this state is about — or `None` when the
    /// file says `null` or says something that cannot be a pid.
    pub spotify_pid: Option<u32>,
    /// The `since` field as a wall-clock time, or `None` when it is absent,
    /// `null`, or not a number of seconds since the epoch.
    pub since: Option<SystemTime>,
}

/// The two questions COMPAT wants answered before a duck is believed, and which
/// of them trak has.
///
/// Both are questions about the machine rather than about the file, and neither
/// can be answered inside this module: Spotify's pid belongs to the player layer
/// and "is a Sonar running" is a process list. [`Trust::Unchecked`] means exactly
/// that — the file's own rules were applied and the machine was not asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// No machine answer available. Shape, version and mtime were checked; the
    /// pid is taken on the file's word. This is what [`read_state`] uses.
    Unchecked,
    /// What the machine could be asked.
    Checked {
        /// Spotify's pid, as the player layer reports it. `None` while Spotify is
        /// restarting or AppleScript is not answering.
        spotify_pid: Option<u32>,
        /// Whether a Sonar process is running. COMPAT accepts this in place of a
        /// fresh mtime, because a Sonar that is alive is the thing that would
        /// have rewritten the file.
        sonar_alive: bool,
    },
}

/// What trak believes about Sonar, after a file has been read and checked.
///
/// Every reason a file is not believed arrives here as [`SonarState::unknown`],
/// so the TUI has one thing to test and no error to swallow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SonarState {
    /// The phase trak believes, or [`SonarPhase::Unknown`].
    pub phase: SonarPhase,
    /// When that phase began, as the file said. `None` when there was no usable
    /// `since`, which does not make the state untrusted: the phase is what trak
    /// acts on, the age is only what a notice shows.
    pub since: Option<SystemTime>,
}

impl SonarState {
    /// Nothing usable. The answer to a missing file, an unreadable one, a bad
    /// document, an unknown version, a pid that is not this Spotify, and a file
    /// older than [`FRESH_FOR`].
    pub fn unknown() -> Self {
        SonarState {
            phase: SonarPhase::Unknown,
            since: None,
        }
    }

    /// Whether a Sonar was found and believed. COMPAT's badge hangs off this, so
    /// a Sonar that is not there takes its badge with it rather than leaving a
    /// stale one on screen.
    pub fn is_known(&self) -> bool {
        self.phase != SonarPhase::Unknown
    }

    /// Whether Sonar is holding the volume right now: `ducking`, `ducked` or
    /// `resuming`. This is the flag that disables `m` and that says a mid-fade
    /// volume reading is not the user's volume.
    pub fn is_ducking(&self) -> bool {
        self.phase.is_duck()
    }

    /// The header badge, or `None` when there is no Sonar to show one for.
    ///
    /// Lower-case, like the `headless` badge, and deliberately the same in every
    /// phase: whether the music is paused is the pause line's business, not the
    /// badge's.
    pub fn badge(&self) -> Option<&'static str> {
        match self.phase {
            SonarPhase::Unknown => None,
            SonarPhase::Idle | SonarPhase::Ducking | SonarPhase::Ducked | SonarPhase::Resuming => {
                Some("sonar")
            }
        }
    }

    /// One line for a toast, using the clock.
    pub fn notice(&self) -> String {
        self.notice_at(SystemTime::now())
    }

    /// [`SonarState::notice`] as of `now`, so the age in it is testable without
    /// sleeping.
    ///
    /// Never contains a newline: the text is fixed and the only variable part is
    /// an integer and a unit, so it fits a status line whatever the file said.
    pub fn notice_at(&self, now: SystemTime) -> String {
        let line = match self.phase {
            SonarPhase::Unknown => return "sonar: not running".to_string(),
            SonarPhase::Idle => return "sonar: not ducking".to_string(),
            SonarPhase::Ducking => "sonar: ducking, volume fading down",
            SonarPhase::Ducked => "sonar: ducked, auto-paused",
            SonarPhase::Resuming => "sonar: resuming, volume fading up",
        };
        // The age is on a duck and not on an idle Sonar because it is the whole
        // difference between "Sonar is fading right now" and "Sonar has been
        // holding this for four minutes", and because `idle` also covers a Sonar
        // that is not here at all.
        match self.duck_age(now) {
            Some(age) => format!("{line} for {}", human_age(age)),
            None => line.to_string(),
        }
    }

    /// How long the duck has been under way, or `None` when the file carried no
    /// usable `since`, when nothing is being ducked, or when `since` is in the
    /// future — a clock that disagrees with the writer's, which is not an age.
    fn duck_age(&self, now: SystemTime) -> Option<Duration> {
        if !self.is_ducking() {
            return None;
        }
        now.duration_since(self.since?).ok()
    }
}

/// An age in the largest unit that keeps it short, so a notice stays a line:
/// `8s`, `4m`, `2h`, `3d`. Division only, so a `since` far in the past cannot
/// overflow into anything.
fn human_age(age: Duration) -> String {
    let secs = age.as_secs();
    if secs < 60 {
        return format!("{secs}s");
    }
    if secs < 3_600 {
        return format!("{}m", secs / 60);
    }
    if secs < 86_400 {
        return format!("{}h", secs / 3_600);
    }
    format!("{}d", secs / 86_400)
}

/// Read one state file and report every field it carries.
///
/// The shape is the only thing judged here: a `v` trak does not know is still
/// [`SonarReport::version`] rather than an error, because deciding what to do
/// about it is [`resolve`]'s job and keeping the two apart is what makes both
/// testable on their own.
///
/// `v` and `state` are required, since without them the file says nothing trak
/// could act on. `pid` and `since` are not: Sonar writes `null` for a pid it does
/// not have, and a file without an age is still a state.
pub fn parse(bytes: &[u8]) -> Result<SonarReport, ParseError> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ParseError::TooLarge {
            bytes: bytes.len() as u64,
        });
    }
    let json = parse_json(bytes)?;
    let version = json.integer("v").ok_or(ParseError::Malformed)?;
    // A `state` with no matching word is refused rather than guessed at. Guessing
    // `idle` would hide a duck trak must not fight; guessing `ducked` would
    // disable `m` and pin a badge for a phase that may mean something else
    // entirely. Matching is exact, so a `v` bump is the way to change the words.
    let phase = json
        .text("state")
        .and_then(phase_for)
        .ok_or(ParseError::Malformed)?;
    // A pid that cannot be one — beyond a `u32`, or written as a float, or as a
    // string — is dropped rather than repaired: the check that matters happens in
    // `vouches`, and a made-up pid would pass it.
    let spotify_pid = json.integer("pid").and_then(|pid| u32::try_from(pid).ok());
    let since = json
        .integer("since")
        .and_then(|secs| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs)));
    Ok(SonarReport {
        version,
        phase,
        spotify_pid,
        since,
    })
}

/// Sonar's four state names.
fn phase_for(name: &str) -> Option<SonarPhase> {
    Some(match name {
        "idle" => SonarPhase::Idle,
        "ducking" => SonarPhase::Ducking,
        "ducked" => SonarPhase::Ducked,
        "resuming" => SonarPhase::Resuming,
        _ => return None,
    })
}

/// Whether a file modified at `mtime` is recent enough to be believed at `now`.
///
/// The boundary is inside the window: a file exactly [`FRESH_FOR`] old is still
/// taken, because the next poll sees it a second younger and a rule that flipped
/// on the boundary would make the badge flicker.
///
/// An mtime in the future counts as fresh. Clocks disagree, and a writer that
/// stamped something later than trak's clock has not stopped writing — that is a
/// different problem from the one the window exists for.
pub fn is_fresh(mtime: SystemTime, now: SystemTime) -> bool {
    match now.duration_since(mtime) {
        Ok(age) => age <= FRESH_FOR,
        // The difference cannot be represented, which for real files means the
        // mtime is after `now` rather than absurdly far from it.
        Err(_) => true,
    }
}

/// Apply trak's rules to a parsed file: the version, then the mtime and the
/// witness.
///
/// Pure, and taking `now` as an argument, so the whole policy — including the
/// staleness that would otherwise need a ten-minute-old file — is a matter of
/// passing different times.
pub fn resolve(
    report: SonarReport,
    mtime: SystemTime,
    now: SystemTime,
    trust: Trust,
) -> SonarState {
    // An unknown version is refused rather than read as version 1: the fields trak
    // depends on are the ones a future version is free to have changed.
    if report.version != KNOWN_VERSION {
        return SonarState::unknown();
    }
    if !vouches(trust, &report, mtime, now) {
        return SonarState::unknown();
    }
    SonarState {
        phase: report.phase,
        since: report.since,
    }
}

/// COMPAT's two machine-side rules: the file has to be about this Spotify, and it
/// has to be fresh or still being written.
fn vouches(trust: Trust, report: &SonarReport, mtime: SystemTime, now: SystemTime) -> bool {
    let (spotify_pid, sonar_alive) = match trust {
        // No machine answer, so the mtime is the whole of the freshness rule. It
        // is a fact about the file rather than about the machine, so it holds
        // either way: leaving it out would let `read_state` believe a duck from a
        // file a dead Sonar left hours ago.
        Trust::Unchecked => return is_fresh(mtime, now),
        Trust::Checked {
            spotify_pid,
            sonar_alive,
        } => (spotify_pid, sonar_alive),
    };
    let about_this_spotify = match report.spotify_pid {
        Some(pid) => spotify_pid == Some(pid),
        // Sonar writes `null` when it has no Spotify to watch. There is nothing
        // there to check, so the live process is the evidence COMPAT accepts in
        // its place — and with no process and no pid, the file is a rumour.
        None => sonar_alive,
    };
    // Fresh, or still being written: a Sonar that is alive is the thing that
    // would have rewritten the file, so its silence is not evidence.
    about_this_spotify && (is_fresh(mtime, now) || sonar_alive)
}

/// Read `path` and apply every rule trak can apply from the file alone: the
/// shape, the version, and the mtime.
///
/// COMPAT's pid rule needs Spotify's pid and a process list, which belong to the
/// caller; [`read_state_trusted`] is the form that applies it. Everything this
/// misses still shows the `Sonar` badge, so a Sonar that is there is never
/// invisible — only a duck is left unguarded, and the volume trak writes is the
/// user's own last-set value either way.
///
/// Never fails. A few hundred bytes from a file Sonar writes atomically is not
/// work that belongs on another thread, and every way of not getting them is
/// [`SonarState::unknown`].
pub fn read_state(path: &Path) -> SonarState {
    read_state_trusted(path, Trust::Unchecked)
}

/// [`read_state`] plus COMPAT's pid rule: a file written for a Spotify that has
/// quit is not a claim about the one on screen, and a file nobody is rewriting
/// is not a claim about now.
pub fn read_state_trusted(path: &Path, trust: Trust) -> SonarState {
    match read_file(path) {
        Some((report, mtime)) => resolve(report, mtime, SystemTime::now(), trust),
        None => SonarState::unknown(),
    }
}

/// The file and its mtime, or `None` for every way of not having them.
fn read_file(path: &Path) -> Option<(SonarReport, SystemTime)> {
    let file = File::open(path).ok()?;
    // Metadata from the open handle, not from a second lookup of the path: Sonar
    // writes atomically (temp file + rename), so the bytes and the mtime have to
    // come from the same file or the pair describes two different writes.
    let meta = file.metadata().ok()?;
    let mtime = meta.modified().ok()?;
    if meta.len() > MAX_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    // The stat is a cheap filter, not the limit: a file that grows between the
    // two reads is still bounded here, and still bounded again in `parse`.
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
    Some((parse(&bytes).ok()?, mtime))
}

/// Just enough JSON for one four-field file.
///
/// The same shape, and the same strictness, as the reader in `lyrics.rs`, for the
/// same reason: the crate has no serde in it, and a dependency for one flat object
/// of a number, two words and a timestamp is not worth it. The document has to be
/// exactly one value, a number has to be a number, and a truncated or over-deep
/// body is refused rather than half-read.
///
/// The one difference: a number's text is kept. Nothing trak shows is a number,
/// but `v`, `pid` and `since` are numbers it has to have.
#[derive(Debug)]
enum Json {
    Null,
    // The reader has to understand every JSON value, not only the four trak
    // reads, so that an unknown field of any shape is skipped rather than
    // refused. These two are that skipping: a state file has no boolean and no
    // array in it, and nothing to look up in one.
    #[allow(
        dead_code,
        reason = "parsed so an unknown field of either shape is skipped"
    )]
    Bool(bool),
    /// The token as it was written, once it has been checked to be a number. Text
    /// rather than an `f64` so that a version, a pid and a second count are read
    /// as the integers they have to be instead of as floats to be rounded.
    Num(String),
    Str(String),
    #[allow(
        dead_code,
        reason = "parsed so an unknown field of either shape is skipped"
    )]
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        let Json::Obj(fields) = self else { return None };
        fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// A string field, treating empty as absent: Sonar has four state words and
    /// none of them is the empty one.
    fn text(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(Json::Str(s)) if !s.is_empty() => Some(s),
            _ => None,
        }
    }

    /// A field as a whole number. `None` for a missing field, a `null`, a string,
    /// and a number that is not an integer: `1.0` and `-1` are things Sonar does
    /// not write, and rounding either to the integer beside it would be a guess
    /// about a version or a pid.
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
        // The bytes came from a file and only escapes were added, so this can only
        // fail on a file that is not UTF-8, which the branches below do not rule
        // out. It is checked anyway, because a panic in the poll is never
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
            // A state word is ASCII, but a file from a future Sonar may carry
            // anything, and the same rule as `lyrics.rs` applies: a character
            // outside the basic plane is two escapes, and only a well-formed pair
            // is one character.
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
                // character behind it, and guessing one is how a word turns into
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
        // Checked and kept: a token that is not a number is a broken file, and
        // skipping it without checking would let a document through as one whose
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
    use std::collections::HashSet;
    use std::fs;

    /// A state file exactly as AGENT-PROMPTS prompt 1 pins it.
    const STATE: &str = r#"{"v":1,"state":"ducking","pid":501,"since":1790000000}"#;

    /// The pid in `STATE`: a Spotify, not a Sonar.
    const SPOTIFY: u32 = 501;

    /// The Unix second `STATE` claims, as a time.
    fn since() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000)
    }

    /// An arbitrary "now", far enough from `since()` that the ages in notices are
    /// readable. Every time in these tests is passed in, never slept for.
    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn ago(secs: u64) -> SystemTime {
        now() - Duration::from_secs(secs)
    }

    fn parsed(json: &str) -> SonarReport {
        parse(json.as_bytes()).unwrap_or_else(|e| panic!("{json:?} should parse: {e}"))
    }

    /// A witness that matches `STATE`: this Spotify, and a Sonar that is running.
    fn witness() -> Trust {
        Trust::Checked {
            spotify_pid: Some(SPOTIFY),
            sonar_alive: true,
        }
    }

    #[test]
    fn a_state_file_parses_with_every_field() {
        let report = parsed(STATE);
        assert_eq!(report.version, 1);
        assert_eq!(report.phase, SonarPhase::Ducking);
        assert_eq!(report.spotify_pid, Some(SPOTIFY));
        assert_eq!(report.since, Some(since()));
    }

    #[test]
    fn each_of_sonar_s_four_states_is_read() {
        for (name, phase) in [
            ("idle", SonarPhase::Idle),
            ("ducking", SonarPhase::Ducking),
            ("ducked", SonarPhase::Ducked),
            ("resuming", SonarPhase::Resuming),
        ] {
            let json = format!(r#"{{"v":1,"state":"{name}","pid":501,"since":1790000000}}"#);
            assert_eq!(parsed(&json).phase, phase, "{name}");
        }
    }

    #[test]
    fn the_three_ducks_are_distinguished() {
        let phases = [
            SonarPhase::Ducking,
            SonarPhase::Ducked,
            SonarPhase::Resuming,
            SonarPhase::Idle,
            SonarPhase::Unknown,
        ];
        for phase in phases {
            let state = SonarState {
                phase,
                since: Some(ago(6)),
            };
            assert_eq!(state.is_ducking(), phase.is_duck(), "{phase:?}");
            assert_eq!(state.is_known(), phase != SonarPhase::Unknown, "{phase:?}");
        }
        // Each duck says something different, because a fade down, a pause and a
        // fade back up are three different things to have happened to the volume.
        let notices: HashSet<String> = [
            SonarPhase::Ducking,
            SonarPhase::Ducked,
            SonarPhase::Resuming,
        ]
        .iter()
        .map(|phase| {
            SonarState {
                phase: *phase,
                since: Some(ago(6)),
            }
            .notice_at(now())
        })
        .collect();
        assert_eq!(notices.len(), 3, "{notices:?}");
    }

    #[test]
    fn fields_trak_has_never_heard_of_are_ignored() {
        let json = concat!(
            r#"{"v":1,"state":"ducked","pid":501,"since":1790000000,"#,
            r#""note":"written by a newer Sonar","engine":{"x":1},"#,
            r#""levels":[0.5,0.25],"since_ms":1790000000000,"since":null,"#,
            r#""pausedBy":null,"track":{"id":"abc"}}"#
        );
        let report = parsed(json);
        assert_eq!(report.version, 1);
        assert_eq!(report.phase, SonarPhase::Ducked);
        assert_eq!(report.spotify_pid, Some(SPOTIFY));
        assert_eq!(
            report.since,
            Some(since()),
            "the first of a repeated key wins"
        );
    }

    #[test]
    fn a_duck_needs_no_pid_and_no_since_to_be_believed() {
        // Sonar writes `null` for a pid it does not have, and a file with no
        // `since` is still a state trak can act on.
        let report = parsed(r#"{"v":1,"state":"ducked","pid":null}"#);
        assert_eq!(report.phase, SonarPhase::Ducked);
        assert_eq!(report.spotify_pid, None);
        assert_eq!(report.since, None);
        let state = resolve(report, now(), now(), witness());
        assert_eq!(state.phase, SonarPhase::Ducked);
        assert!(state.is_ducking());
        assert_eq!(state.badge(), Some("sonar"));
    }

    #[test]
    fn an_unknown_version_is_refused_rather_than_guessed_at() {
        for version in [0, 2, 7, 99, u64::MAX] {
            let json =
                format!(r#"{{"v":{version},"state":"ducking","pid":501,"since":1790000000}}"#);
            // The report is faithful to the file...
            let report = parsed(&json);
            assert_eq!(report.version, version);
            assert_eq!(report.phase, SonarPhase::Ducking);
            // ...and the state refuses it either way round, checked or not.
            assert_eq!(
                resolve(report, now(), now(), witness()),
                SonarState::unknown(),
                "v {version}"
            );
            assert_eq!(
                resolve(report, now(), now(), Trust::Unchecked),
                SonarState::unknown(),
                "v {version}"
            );
        }
    }

    #[test]
    fn a_version_that_is_not_a_number_is_malformed() {
        for v in [
            r#""v":1.0"#.to_string(),
            r#""v":null"#.to_string(),
            r#""v":-1"#.to_string(),
            r#""v":"1""#.to_string(),
            r#""v":[]"#.to_string(),
            r#""v":true"#.to_string(),
            format!(r#""v":{}"#, "9".repeat(400)), // far beyond a u64
        ] {
            let json = format!("{{{v},\"state\":\"idle\"}}");
            assert!(
                matches!(parse(json.as_bytes()), Err(ParseError::Malformed)),
                "{json} should be malformed"
            );
        }
        // And a file with no `v` at all.
        assert!(matches!(
            parse(br#"{"state":"idle"}"#),
            Err(ParseError::Malformed)
        ));
    }

    #[test]
    fn a_pid_from_another_spotify_is_refused() {
        for other in [0, 1, 500, 502, 999_999] {
            let state = resolve(
                parsed(STATE),
                now(),
                now(),
                Trust::Checked {
                    spotify_pid: Some(other),
                    sonar_alive: true,
                },
            );
            assert_eq!(state, SonarState::unknown(), "pid {other}");
        }
        // The same file with this Spotify is believed.
        let state = resolve(parsed(STATE), now(), now(), witness());
        assert_eq!(state.phase, SonarPhase::Ducking);
        // A file whose Spotify is unknown is not a duck, even with a live Sonar:
        // there is nothing to compare and the pid is the check COMPAT asks for.
        let state = resolve(
            parsed(STATE),
            now(),
            now(),
            Trust::Checked {
                spotify_pid: None,
                sonar_alive: true,
            },
        );
        assert_eq!(state, SonarState::unknown());
    }

    #[test]
    fn a_null_pid_is_taken_on_a_live_sonar() {
        // Sonar could not read a pid, so COMPAT's alternative evidence applies.
        let report = parsed(r#"{"v":1,"state":"ducking","pid":null,"since":1790000000}"#);
        for (alive, believed) in [(true, true), (false, false)] {
            let state = resolve(
                report,
                now(),
                now(),
                Trust::Checked {
                    spotify_pid: Some(SPOTIFY),
                    sonar_alive: alive,
                },
            );
            assert_eq!(state.is_known(), believed, "alive {alive}");
        }
    }

    #[test]
    fn an_idle_sonar_still_needs_the_pid_to_match() {
        // `idle` is written on quit as well as when Auto-Pause is off, so an
        // unreadable file must not show a Sonar badge for a Spotify that is gone.
        let report = parsed(r#"{"v":1,"state":"idle","pid":501}"#);
        let other = resolve(
            report,
            now(),
            now(),
            Trust::Checked {
                spotify_pid: Some(999),
                sonar_alive: true,
            },
        );
        assert_eq!(other, SonarState::unknown());
        let same = resolve(
            report,
            now(),
            now(),
            Trust::Checked {
                spotify_pid: Some(SPOTIFY),
                sonar_alive: true,
            },
        );
        assert_eq!(same.phase, SonarPhase::Idle);
        assert!(!same.is_ducking());
        assert_eq!(same.badge(), Some("sonar"));
    }

    #[test]
    fn a_stale_file_is_stale_and_a_fresh_one_is_not() {
        // The window itself, from COMPAT: ten minutes.
        assert_eq!(FRESH_FOR, Duration::from_secs(600));
        assert!(is_fresh(ago(0), now()), "just written");
        assert!(is_fresh(ago(599), now()), "just inside");
        assert!(is_fresh(ago(600), now()), "exactly on the boundary");
        assert!(!is_fresh(ago(601), now()), "just outside");
        assert!(!is_fresh(ago(86_400), now()), "a day old");
        assert!(!is_fresh(SystemTime::UNIX_EPOCH, now()), "the epoch");
        // A clock that disagrees is not a stale file.
        assert!(
            is_fresh(now() + Duration::from_secs(5), now()),
            "mtime ahead"
        );
        assert!(is_fresh(now() + Duration::from_secs(86_400), now()));
    }

    #[test]
    fn a_stale_file_is_refused_and_a_live_sonar_still_writes_it() {
        // Nothing but the mtime says a duck is over.
        let stale = resolve(
            parsed(STATE),
            ago(11 * 60),
            now(),
            Trust::Checked {
                spotify_pid: Some(SPOTIFY),
                sonar_alive: false,
            },
        );
        assert_eq!(stale, SonarState::unknown(), "a dead Sonar's leftover file");
        // COMPAT's alternative: a Sonar that is alive is the thing that would
        // have rewritten it.
        let alive = resolve(
            parsed(STATE),
            ago(11 * 60),
            now(),
            Trust::Checked {
                spotify_pid: Some(SPOTIFY),
                sonar_alive: true,
            },
        );
        assert_eq!(alive.phase, SonarPhase::Ducking);
        // Freshness does not rescue a file about another Spotify.
        let other = resolve(
            parsed(STATE),
            ago(11 * 60),
            now(),
            Trust::Checked {
                spotify_pid: Some(SPOTIFY + 1),
                sonar_alive: true,
            },
        );
        assert_eq!(other, SonarState::unknown());
    }

    #[test]
    fn the_mtime_rule_applies_even_when_the_machine_was_not_asked() {
        // `read_state` cannot check the pid, but the file's own rules still hold.
        for mtime in [ago(11 * 60), ago(86_400), SystemTime::UNIX_EPOCH] {
            assert_eq!(
                resolve(parsed(STATE), mtime, now(), Trust::Unchecked),
                SonarState::unknown(),
                "stale at {mtime:?}"
            );
        }
        assert_eq!(
            resolve(parsed(STATE), ago(3), now(), Trust::Unchecked).phase,
            SonarPhase::Ducking
        );
    }

    #[test]
    fn rubbish_is_malformed_rather_than_a_panic() {
        for body in [
            "{",
            "[]",
            "null",
            "1",
            "true",
            "\"ducking\"",
            "",
            "   ",
            "{}",
            r#"{"v":1"#,
            r#"{"v":1,"state":"ducking""#,
            r#"{"v":1,"state":"duck"#,
            r#"{"v":1,"state":"duc\king"#,
            r#"{"v":1,"state":"ducking","pid":}"#,
            r#"{"v":1,"state":"ducking"} trailing"#,
            r#"{"v":1,"state":"ducking"}{"#,
            r#"{"v":1,,"state":"idle"}"#,
            r#"{v:1,"state":"idle"}"#,
            r#"{"v":1"state":"idle"}"#,
            r#"{"v":1 "state":"idle"}"#,
            r#"{"v":1.2.3,"state":"idle"}"#,
            r#"{"v":1,"state":3}"#,
            r#"{"v":1,"state":null}"#,
            r#"{"v":1,"state":""}"#,
            r#"{"v":1,"state":[]}"#,
            r#"{"v":1,"state":{"phase":"ducking"}}"#,
            // A state word trak has no case rule for.
            r#"{"v":1,"state":"Ducking"}"#,
            r#"{"v":1,"state":"ducked "}"#,
            r#"{"v":1,"state":" ducking"}"#,
            r#"{"v":1,"state":"duc\u004bbing"}"#,
            r#"{"v":1,"state":"ducked\n"}"#,
            r#"{"v":1,"state":"\uD83C"}"#,
            r#"{"v":1,"state":"\q"}"#,
            r#"{"v":1,"state":"unterminated}"#,
            "not json at all",
            // A byte-order mark, which a writer is free to add and this reader
            // is not: refused rather than skipped, because skipping it would mean
            // guessing where a document starts.
            "\u{feff}{\"v\":1,\"state\":\"idle\"}",
        ] {
            assert!(
                matches!(parse(body.as_bytes()), Err(ParseError::Malformed)),
                "{body:?} should be malformed"
            );
        }
    }

    #[test]
    fn a_truncated_state_file_is_malformed_at_every_cut() {
        // Sonar writes atomically, so trak should never see half a document — but
        // "should never" is not a reason to have an unchecked index.
        for cut in 0..STATE.len() {
            let body = &STATE.as_bytes()[..cut];
            assert!(
                matches!(parse(body), Err(ParseError::Malformed)),
                "cut at {cut} ({:?}) should be malformed",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn a_file_that_is_not_text_is_malformed() {
        for body in [
            &b"{\"v\":1,\"state\":\"\xff\xfe\"}"[..],
            &b"{\"v\":1,\"state\":\"ducking\"}\xff"[..],
            &b"\xff\xfe\xfd"[..],
            // A lone high surrogate, and a high one with nothing after it.
            &br#"{"v":1,"state":"\uD83C"}"#[..],
            &br#"{"v":1,"state":"\uD83C"}[..],
            &br#"{"v":1,"state":"\uD83C\u0041"}"#[..],
        ] {
            assert!(
                matches!(parse(body), Err(ParseError::Malformed)),
                "{:?} should be malformed",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn escapes_and_unicode_are_read_but_still_have_to_be_a_state() {
        // The reader is the same one as `lyrics.rs`', so a file from a future
        // Sonar carrying an escaped word is read rather than refused as broken.
        let report = parsed(r#"{"v":1,"state":"\u0069dle","pid":501}"#);
        assert_eq!(report.phase, SonarPhase::Idle);
        // A non-ASCII word is a word trak has no name for.
        assert!(matches!(
            parse(br#"{"v":1,"state":"d\u00fccking"}"#),
            Err(ParseError::Malformed)
        ));
    }

    #[test]
    fn a_deep_body_is_refused_instead_of_overflowing_the_stack() {
        for body in [
            "[".repeat(2000),
            format!(r#"{{"v":1,"state":"idle","x":{}}}"#, "[".repeat(2000)),
        ] {
            assert!(
                matches!(parse(body.as_bytes()), Err(ParseError::Malformed)),
                "a deep body should be refused"
            );
        }
    }

    #[test]
    fn a_file_larger_than_the_cap_is_refused() {
        // A state file is four fields. Anything bigger is not one, and the poll
        // should not pay to find out what it is.
        let padding = "x".repeat(MAX_BYTES as usize);
        let body = format!(r#"{{"v":1,"state":"idle","note":"{padding}"}}"#);
        assert!(matches!(
            parse(body.as_bytes()),
            Err(ParseError::TooLarge { .. })
        ));
        // And one that is only just under it still parses.
        let short = format!(r#"{{"v":1,"state":"idle","note":"{}"}}"#, "x".repeat(100));
        assert_eq!(parsed(&short).phase, SonarPhase::Idle);
    }

    #[test]
    fn an_absurd_pid_or_since_is_dropped_rather_than_repaired() {
        // A pid past a `u32` is not the pid of anything, and a second count past
        // the end of the calendar is not a time; both are dropped, so neither can
        // be believed by accident.
        let report =
            parsed(r#"{"v":1,"state":"idle","pid":99999999999,"since":18446744073709551615}"#);
        assert_eq!(report.spotify_pid, None, "not a pid anything could have");
        assert_eq!(report.since, None, "no such second count");
        // A pid written as a float or a string is not the integer beside it.
        for pid in [
            r#"501.0"#, r#""501""#, r#"[501]"#, r#"-501"#, r#"null"#, r#"1e3"#,
        ] {
            let json = format!(r#"{{"v":1,"state":"idle","pid":{pid}}}"#);
            assert_eq!(parsed(&json).spotify_pid, None, "pid {pid}");
        }
        // The largest real second count still becomes a time.
        let report = parsed(r#"{"v":1,"state":"idle","since":4102444800}"#);
        assert_eq!(
            report.since,
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(4_102_444_800))
        );
    }

    #[test]
    fn a_missing_file_is_a_state_and_not_an_error() {
        let dir = TempDir::new("missing");
        let gone = dir.path().join("state.json");
        assert_eq!(read_state(&gone), SonarState::unknown());
        assert_eq!(read_state_trusted(&gone, witness()), SonarState::unknown());

        // A path that is a directory, an empty file and a file of noise are the
        // same answer.
        assert_eq!(read_state(dir.path()), SonarState::unknown());
        let empty = dir.path().join("empty.json");
        fs::write(&empty, b"").expect("write the empty file");
        assert_eq!(read_state(&empty), SonarState::unknown());
        let noise = dir.path().join("noise.json");
        fs::write(&noise, b"{oh dear").expect("write the noise file");
        assert_eq!(read_state(&noise), SonarState::unknown());
    }

    #[test]
    fn a_state_file_on_disk_is_read() {
        let dir = TempDir::new("ondisk");
        let path = dir.path().join("state.json");
        fs::write(&path, STATE).expect("write the state file");

        // Just written, so its mtime is now and the freshness rule passes on the
        // real clock: this is the only test that reads the clock, and it is the
        // one that has to.
        let state = read_state(&path);
        assert_eq!(state.phase, SonarPhase::Ducking);
        assert_eq!(state.since, Some(since()));
        assert!(state.is_ducking());
        assert_eq!(state.badge(), Some("sonar"));

        // The full form, with this Spotify and a live Sonar.
        assert_eq!(
            read_state_trusted(&path, witness()).phase,
            SonarPhase::Ducking
        );
        // And refused for another Spotify, which is the rule `read_state` alone
        // cannot apply.
        assert_eq!(
            read_state_trusted(
                &path,
                Trust::Checked {
                    spotify_pid: Some(SPOTIFY + 1),
                    sonar_alive: true,
                }
            ),
            SonarState::unknown()
        );
        // A file this long ago is not read: the mtime is set rather than waited
        // for, so the test is the rule and not the clock. It has to be the real
        // clock here — `read_state` reads it — so the mtime is set relative to
        // `SystemTime::now()` rather than to the fixed instant above.
        let old = File::options()
            .write(true)
            .open(&path)
            .expect("open for the mtime");
        let stale = SystemTime::now() - Duration::from_secs(11 * 60);
        old.set_modified(stale).expect("set the mtime");
        drop(old);
        assert_eq!(
            read_state_trusted(&path, witness()).phase,
            SonarPhase::Ducking
        ); // Sonar alive
        assert_eq!(read_state(&path), SonarState::unknown(), "stale on its own");
    }

    #[test]
    fn a_notice_says_how_long_a_duck_has_been_going_on() {
        let state = |phase| SonarState {
            phase,
            since: Some(ago(0)),
        };
        // Seconds, then the largest unit that keeps it short.
        for (secs, tail) in [
            (0_u64, "for 0s"),
            (1, "for 1s"),
            (59, "for 59s"),
            (60, "for 1m"),
            (3_599, "for 59m"),
            (3_600, "for 1h"),
            (86_399, "for 23h"),
            (86_400, "for 1d"),
        ] {
            let notice = SonarState {
                phase: SonarPhase::Ducked,
                since: Some(now() - Duration::from_secs(secs)),
            }
            .notice_at(now());
            assert!(
                notice.ends_with(tail),
                "at {secs}s: {notice:?} should end with {tail:?}"
            );
            assert!(notice.starts_with("sonar: ducked"), "{notice:?}");
        }
        // A `since` at the far end of the range still counts up in days and still
        // produces one line, rather than overflowing or printing a duration's
        // debug form.
        let absurd = SonarState {
            phase: SonarPhase::Ducked,
            since: Some(now() - Duration::from_secs(u64::MAX / 2)),
        }
        .notice_at(now());
        assert!(absurd.ends_with('d'), "{absurd:?}");
        assert!(!absurd.contains('\n'), "{absurd:?}");
        // No `since` leaves the line short rather than printing an age of zero.
        assert_eq!(
            SonarState {
                phase: SonarPhase::Ducking,
                since: None
            }
            .notice_at(now()),
            "sonar: ducking, volume fading down"
        );
        // A `since` in the future is a clock that disagrees, not an age.
        assert_eq!(
            SonarState {
                phase: SonarPhase::Ducking,
                since: Some(now() + Duration::from_secs(60)),
            }
            .notice_at(now()),
            "sonar: ducking, volume fading down"
        );
        // An idle Sonar never counts up: `idle` also covers "Sonar is not here".
        assert_eq!(
            state(SonarPhase::Idle).notice_at(now()),
            "sonar: not ducking"
        );
        assert_eq!(SonarState::unknown().notice_at(now()), "sonar: not running");
    }

    #[test]
    fn every_notice_and_badge_is_one_line() {
        for phase in [
            SonarPhase::Unknown,
            SonarPhase::Idle,
            SonarPhase::Ducking,
            SonarPhase::Ducked,
            SonarPhase::Resuming,
        ] {
            // An age, no age, and an age that cannot be computed.
            for since in [None, Some(ago(0)), Some(ago(9_000)), Some(now())] {
                let state = SonarState { phase, since };
                let notice = state.notice_at(now());
                assert!(!notice.is_empty(), "{state:?}");
                assert!(!notice.contains('\n'), "one line: {notice:?}");
                assert!(!notice.contains('\r'), "one line: {notice:?}");
                assert!(!notice.contains("panicked"), "{notice:?}");
                let badge = state.badge();
                if let Some(badge) = badge {
                    assert!(!badge.is_empty(), "{state:?}");
                    assert!(!badge.contains('\n'), "one line: {badge:?}");
                }
                // The badge is there for every believed state and gone for
                // nothing, which is the whole of COMPAT's badge rule.
                assert_eq!(badge.is_some(), state.is_known(), "{state:?}");
            }
        }
        // And the errors, which are only ever shown in a debug status.
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
        let unknown = SonarState::unknown();
        assert_eq!(unknown.phase, SonarPhase::Unknown);
        assert_eq!(unknown.since, None);
        assert!(!unknown.is_known());
        assert!(!unknown.is_ducking());
        assert_eq!(unknown.badge(), None);
        // And it is what every refused file resolves to, so there is one answer to
        // look for rather than a reason to display.
        assert_eq!(
            resolve(
                parsed(r#"{"v":2,"state":"ducking","pid":501}"#),
                now(),
                now(),
                witness()
            ),
            unknown
        );
        assert_eq!(
            resolve(
                parsed(r#"{"v":1,"state":"idle","pid":501}"#),
                ago(11 * 60),
                now(),
                Trust::Checked {
                    spotify_pid: Some(SPOTIFY),
                    sonar_alive: false,
                }
            ),
            unknown
        );
    }

    #[test]
    fn the_header_line_a_duck_gets_is_one_line_of_its_own() {
        // COMPAT: `⏸ auto-paused by Sonar` in the header, for every duck phase. The
        // words come from the notice and the glyph from the header, so all that is
        // checked here is that no duck produces an empty or wrapped line.
        for phase in [
            SonarPhase::Ducking,
            SonarPhase::Ducked,
            SonarPhase::Resuming,
        ] {
            let state = SonarState {
                phase,
                since: Some(since()),
            };
            let line = state.notice_at(now());
            assert!(line.starts_with("sonar: "), "{line:?}");
            assert!(!line.contains('\n'), "{line:?}");
            assert!(state.is_ducking(), "{phase:?}");
        }
    }

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "trak-sonar-test-{tag}-{}-{:?}",
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
