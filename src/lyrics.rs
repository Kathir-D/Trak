//! Lyrics: a lookup against LRCLIB, and the parsing that makes the answer usable.
//!
//! The network and the format are kept apart on purpose. [`fetch`] blocks, so it
//! belongs on a worker thread and comes back as an event; everything it feeds —
//! [`parse_synced`], [`plain_lines`], [`index_at`] — is a pure function of a
//! string, so the part that is actually hard (LRCLIB's line format) is tested
//! without a socket.
//!
//! The format is one string of lines like `[00:19.16] When you were here
//! before`, and it is not dependable enough to parse strictly. Lines are not
//! always in order, the space after the tag is sometimes missing, and an
//! instrumental break is a bare `[03:50.78]` — a musical rest, not a missing
//! line. Each of those is handled here so the render loop never has to.

use std::fmt::Write as _;
use std::time::Duration;

/// The LRCLIB origin. A constant rather than a parameter so that a song title
/// from Spotify can never influence which host trak talks to.
const ORIGIN: &str = "https://lrclib.net";

/// Give up on a request after this. A hung lookup must not hold a worker slot
/// for the rest of the session, and the same bound `art` uses for a download.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Refuse a body larger than this. Lyrics are kilobytes; anything bigger is not
/// lyrics, and it should not be a way to spend the owner's memory.
const MAX_BYTES: u64 = 1024 * 1024;

/// LRCLIB rejects a `duration` outside 1..=3600 with a 400, and it holds no row
/// for a track that long, so an out-of-range duration is dropped rather than sent.
const MAX_DURATION_SECS: u64 = 3600;

/// How deep a JSON document may nest. LRCLIB sends a flat object two levels deep;
/// the cap is so that a hostile or broken body cannot drive the parser into a
/// stack overflow on the worker thread.
const MAX_DEPTH: usize = 32;

/// LRCLIB asks callers to identify themselves and documents 429 and 503 under
/// load. There is no key and no quota to spend, so a version and a URL is the
/// whole courtesy.
const USER_AGENT: &str = concat!(
    "trak/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/Kathir-D/trak)"
);

/// One line of lyrics.
///
/// `time_secs` is the offset from the start of the track in seconds, or
/// [`f64::NAN`] when the lyrics are unsynced and no line has a time at all. A
/// NaN compares false against everything, so it can never be mistaken for a real
/// timestamp by a comparison; see [`index_at`].
#[derive(Debug, Clone, PartialEq)]
pub struct LyricLine {
    /// Seconds from the start of the track, or [`f64::NAN`] for unsynced lyrics.
    pub time_secs: f64,
    /// The text, with the tag and any space after it removed. Empty for a rest.
    pub text: String,
}

/// The lyrics for one track, or the reason there are none.
#[derive(Debug, Clone, PartialEq)]
pub struct Lyrics {
    /// The lines, in time order. Empty only for an instrumental track.
    pub lines: Vec<LyricLine>,
    /// Whether `lines` carry usable timestamps. `false` for plain lyrics, which
    /// are shown in full rather than scrolled to a position.
    pub synced: bool,
    /// The URL that answered, so the TUI can say where the lyrics came from.
    pub source: String,
    /// The track has no vocals. A real answer with nothing in it, not a failure.
    pub instrumental: bool,
}

/// Why there are no lyrics to show. Every case is a state the TUI renders; none
/// of them is a crash.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LyricsError {
    /// LRCLIB has no row for this track, and the search fallback found nothing
    /// either.
    #[error("no lyrics for this track on LRCLIB")]
    NotFound,
    /// Nothing is listening: no network, no DNS, the connection was refused, or
    /// the body was cut off mid-read. All of those are the same thing to a person
    /// watching a status line.
    #[error("could not reach LRCLIB")]
    Unreachable,
    /// A status trak does not act on. 429 and 503 are how LRCLIB says it is busy
    /// and mean "ask again later", so this is deliberately not retried here: the
    /// worker is not the place to sleep, and the next track change will ask again.
    #[error("LRCLIB answered HTTP {0}")]
    Status(u16),
    /// The status said success and the body was not the JSON it claimed to be.
    #[error("LRCLIB sent an answer trak could not read")]
    Malformed,
    /// No answer within [`TIMEOUT`].
    #[error("LRCLIB did not answer in time")]
    Timeout,
}

impl LyricsError {
    /// One line, for a toast: no newline, no stack trace, no detail that would
    /// need scrolling to read.
    pub fn notice(&self) -> String {
        match self {
            LyricsError::NotFound => "no lyrics for this track".to_string(),
            LyricsError::Unreachable => "lyrics: LRCLIB is unreachable".to_string(),
            LyricsError::Status(code) => format!("lyrics: LRCLIB answered HTTP {code}"),
            LyricsError::Malformed => {
                "lyrics: LRCLIB sent an answer trak could not read".to_string()
            }
            LyricsError::Timeout => "lyrics: LRCLIB did not answer in time".to_string(),
        }
    }
}

/// Parse an LRC block into lines in time order.
///
/// Every line is `[mm:ss.xx] text`. A line is dropped rather than guessed at when
/// the tag will not parse, because a wrong timestamp scrolls the wrong words to
/// the wrong moment, and a missing line only loses a line.
///
/// A line with several tags yields one line per tag: the format does not produce
/// them, but it does not forbid them either, and a line trak cannot explain is a
/// line it should not show. A tag with nothing after it is kept as an empty line,
/// which is what an instrumental break looks like.
pub fn parse_synced(synced: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for raw in synced.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(after) = rest.strip_prefix('[')
            && let Some((tag, tail)) = after.split_once(']')
            && let Some(secs) = parse_tag(tag)
        {
            times.push(secs);
            rest = tail.trim();
        }
        if times.is_empty() {
            continue;
        }
        let text = rest.to_string();
        lines.extend(times.into_iter().map(|time_secs| LyricLine {
            time_secs,
            text: text.clone(),
        }));
    }
    // LRCLIB's rows are not always in order, and the render walks this list
    // assuming they are. `sort_by` is stable, so two lines at the same timestamp
    // keep the order the file had.
    lines.sort_by(|a, b| a.time_secs.total_cmp(&b.time_secs));
    lines
}

/// `mm:ss.xx` to seconds.
///
/// The fraction is always two digits in the data, but it is read as a decimal
/// number rather than counted in hundredths, so `[01:02]`, `[01:02.5]` and
/// `[01:02.50]` all mean 62 seconds. Anything that is not a number — an empty
/// tag, a missing colon, a negative offset — is not a timestamp.
fn parse_tag(tag: &str) -> Option<f64> {
    let (mins, rest) = tag.split_once(':')?;
    // A stray third colon is not something the format has; the part before it is
    // the only sensible reading.
    let mins: f64 = mins.trim().parse().ok()?;
    let secs: f64 = rest.split(':').next()?.trim().parse().ok()?;
    let time = mins * 60.0 + secs;
    (time.is_finite() && time >= 0.0).then_some(time)
}

/// Split unsynced lyrics into lines with no timing.
///
/// Every line gets [`f64::NAN`], which is how a line says "I have no time" and is
/// also what stops [`index_at`] from ever selecting one. Blank lines are dropped:
/// a trailing newline in the data is not a row, and the TUI separates paragraphs
/// itself.
pub fn plain_lines(plain: &str) -> Vec<LyricLine> {
    plain
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|text| LyricLine {
            time_secs: f64::NAN,
            text: text.to_string(),
        })
        .collect()
}

/// The index of the line that should be on screen at `position_secs`: the last one
/// whose time has arrived, which is what a karaoke line means by "current".
///
/// `None` for empty lyrics, before the first line, and for unsynced lyrics — with
/// every time a NaN there is no line to be current, and the TUI shows those in
/// full instead of scrolling them.
///
/// A linear scan, not a binary search: a track has tens of lines rather than
/// thousands, and this runs every frame but has to stay correct for the NaN case
/// without a second code path.
pub fn index_at(lyrics: &Lyrics, position_secs: f64) -> Option<usize> {
    let mut current = None;
    for (i, line) in lyrics.lines.iter().enumerate() {
        if !line.time_secs.is_nan() && line.time_secs <= position_secs {
            current = Some(i);
        }
    }
    current
}

/// Look up lyrics for a track on LRCLIB.
///
/// Blocking: call it from a worker, never from the render loop. `album` and
/// `duration_secs` narrow the match when trak knows them and are simply left out
/// when it does not.
pub fn fetch(
    title: &str,
    artist: &str,
    album: Option<&str>,
    duration_secs: Option<u64>,
) -> Result<Lyrics, LyricsError> {
    fetch_from(ORIGIN, title, artist, album, duration_secs)
}

/// `fetch` against an arbitrary origin.
///
/// The only concession to testability in this module: the tests point it at a
/// loopback server, and production never passes anything but [`ORIGIN`].
fn fetch_from(
    base: &str,
    title: &str,
    artist: &str,
    album: Option<&str>,
    duration_secs: Option<u64>,
) -> Result<Lyrics, LyricsError> {
    let (title, artist) = (title.trim(), artist.trim());
    if title.is_empty() || artist.is_empty() {
        // Nothing to ask about. An advertisement has a title and no lyrics, and a
        // 400 for an empty parameter is not a better answer than "not found".
        return Err(LyricsError::NotFound);
    }

    let mut query = format!(
        "track_name={}&artist_name={}",
        percent_encode(title),
        percent_encode(artist)
    );
    if let Some(album) = album.map(str::trim).filter(|a| !a.is_empty()) {
        let _ = write!(query, "&album_name={}", percent_encode(album));
    }
    if let Some(secs) = duration_secs.filter(|s| (1..=MAX_DURATION_SECS).contains(s)) {
        let _ = write!(query, "&duration={secs}");
    }
    // The endpoint and the query are kept apart: the endpoint is what a person
    // reads in a status line, and it should not carry the track's name with it.
    let endpoint = format!("{base}/api/get");

    match lyrics_at(&format!("{endpoint}?{query}"), &endpoint) {
        // A 404 is LRCLIB saying it has no row for this exact track, not a
        // failure: search is the documented way to find the near match, and a
        // live set is usually the same song from a different release.
        Err(LyricsError::NotFound) => search(base, title, artist),
        other => other,
    }
}

/// The search fallback: the same track, found by name rather than by row.
///
/// A search is a guess — 20 rows, best first, and trak cannot tell which is the
/// right version — so a row with synced lyrics wins over a plain one, and a row
/// with no lyric text at all is not an answer.
fn search(base: &str, title: &str, artist: &str) -> Result<Lyrics, LyricsError> {
    let endpoint = format!("{base}/api/search");
    let url = format!(
        "{endpoint}?q={}",
        percent_encode(&format!("{title} {artist}"))
    );
    let (_, body) = http_get(&url)?;
    let json = parse_json(&body)?;
    let rows = json.as_array().ok_or(LyricsError::Malformed)?;

    let mut plain: Option<&Json> = None;
    for row in rows {
        let synced = row.text("syncedLyrics");
        if synced.is_none() && row.text("plainLyrics").is_none() {
            continue;
        }
        if synced.is_some() {
            return lyrics_from(row, &endpoint);
        }
        if plain.is_none() {
            plain = Some(row);
        }
    }
    plain.map_or(Err(LyricsError::NotFound), |row| {
        lyrics_from(row, &endpoint)
    })
}

/// GET a URL and hand back the status and the body together.
///
/// They come back as a pair because the status decides what the body means: a 400
/// for a missing parameter is *plain text*, so parsing first would turn a typo
/// into "malformed response" and hide the one thing worth saying.
fn http_get(url: &str) -> Result<(u16, String), LyricsError> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        // A 404 is a signal, not an exception: ureq's default throws the body
        // away and hands back only the code.
        .http_status_as_error(false)
        .user_agent(USER_AGENT)
        .build()
        .new_agent();

    let mut response = agent.get(url).call().map_err(transport)?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_string()
        .map_err(transport)?;
    Ok((status, body))
}

/// An `ureq` failure as one of trak's. A timeout is its own case because the fix
/// is different: it is worth retrying on the next track, and a 404 is not.
fn transport(e: ureq::Error) -> LyricsError {
    match e {
        ureq::Error::Timeout(_) => LyricsError::Timeout,
        ureq::Error::StatusCode(code) => LyricsError::Status(code),
        _ => LyricsError::Unreachable,
    }
}

/// One LRCLIB record turned into lyrics, or an error saying it had none.
///
/// The order is synced, then plain, then instrumental, and the last is only
/// reached when there is genuinely no text: a response that claims to be a track
/// and carries no lyrics at all is a broken answer, not an empty one.
fn lyrics_from(row: &Json, source: &str) -> Result<Lyrics, LyricsError> {
    let instrumental = row.flag("instrumental");
    let shell = |synced: bool, lines: Vec<LyricLine>| Lyrics {
        lines,
        synced,
        source: source.to_string(),
        instrumental,
    };

    if let Some(synced) = row.text("syncedLyrics") {
        let lines = parse_synced(synced);
        if !lines.is_empty() {
            return Ok(shell(true, lines));
        }
    }
    if let Some(plain) = row.text("plainLyrics") {
        let lines = plain_lines(plain);
        if !lines.is_empty() {
            return Ok(shell(false, lines));
        }
    }
    if instrumental {
        return Ok(shell(false, Vec::new()));
    }
    Err(LyricsError::Malformed)
}

/// The query for `/api/get`, and the status check that has to happen before the
/// body is read.
fn lyrics_at(url: &str, source: &str) -> Result<Lyrics, LyricsError> {
    let (status, body) = http_get(url)?;
    match status {
        404 => return Err(LyricsError::NotFound),
        200..=299 => {}
        code => return Err(LyricsError::Status(code)),
    }
    lyrics_from(&parse_json(&body)?, source)
}

/// Percent-encode a query value.
///
/// Everything outside the unreserved set is escaped, so a title with a space, an
/// ampersand, a slash or a `#` in it cannot change the shape of the query or
/// truncate the name. A space may legally be sent as a `+`; escaping it is longer
/// and never wrong.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(*byte));
            }
            other => {
                let _ = write!(out, "%{other:02X}");
            }
        }
    }
    out
}

/// Just enough JSON for one endpoint.
///
/// The crate has no serde in it, and LRCLIB's records are a flat object of
/// strings, one boolean and some numbers, so this is smaller than a dependency
/// and it fails the same way on anything that is not the document it expects.
#[derive(Debug)]
enum Json {
    Null,
    Bool(bool),
    /// The number was read and discarded. Nothing trak shows is a number, but a
    /// body whose numbers do not parse is not the JSON it claims to be, and
    /// skipping the token without checking would let it through as a lookup that
    /// found nothing.
    Num,
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// A string field, treating empty as absent. LRCLIB uses `null` for a field
    /// it does not have, and an empty string would produce no lines either way.
    fn text(&self, key: &str) -> Option<&str> {
        self.get(key)
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
    }

    /// A boolean field, false when it is missing or is not a boolean.
    fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some(Json::Bool(true)))
    }

    fn get(&self, key: &str) -> Option<&Json> {
        let Json::Obj(fields) = self else { return None };
        fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(items) => Some(items),
            _ => None,
        }
    }
}

fn parse_json(body: &str) -> Result<Json, LyricsError> {
    let mut p = JsonParser {
        bytes: body.as_bytes(),
        at: 0,
        depth: 0,
    };
    p.skip_space();
    let value = p.value()?;
    p.skip_space();
    // Anything after the document means trak is not reading what it thinks it is.
    if p.at != p.bytes.len() {
        return Err(LyricsError::Malformed);
    }
    Ok(value)
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    at: usize,
    depth: usize,
}

impl JsonParser<'_> {
    fn value(&mut self) -> Result<Json, LyricsError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(LyricsError::Malformed);
        }
        let out = match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') => self.literal(b"true", Json::Bool(true)),
            Some(b'f') => self.literal(b"false", Json::Bool(false)),
            Some(b'n') => self.literal(b"null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(LyricsError::Malformed),
        };
        self.depth -= 1;
        out
    }

    fn object(&mut self) -> Result<Json, LyricsError> {
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
                _ => return Err(LyricsError::Malformed),
            }
        }
    }

    fn array(&mut self) -> Result<Json, LyricsError> {
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
                _ => return Err(LyricsError::Malformed),
            }
        }
    }

    fn string(&mut self) -> Result<String, LyricsError> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            match self.next().ok_or(LyricsError::Malformed)? {
                b'"' => break,
                b'\\' => {
                    let escape = self.next().ok_or(LyricsError::Malformed)?;
                    self.escape(escape, &mut out)?;
                }
                byte => out.push(byte),
            }
        }
        // The bytes came from a `&str` and only escapes were added, so this can
        // only fail on an escape that produced something invalid, which the
        // branches below already rule out. It is checked anyway because a panic
        // on a network body is never acceptable.
        String::from_utf8(out).map_err(|_| LyricsError::Malformed)
    }

    fn escape(&mut self, escape: u8, out: &mut Vec<u8>) -> Result<(), LyricsError> {
        let ch = match escape {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            // Lyrics are full of non-ASCII text and LRCLIB escapes some of it and
            // leaves the rest as UTF-8. Anything outside the basic plane is two
            // escapes, and only a well-formed pair is a character.
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
                // character behind it, and guessing one is how a body turns into
                // replacement characters in the middle of a word.
                char::from_u32(code).ok_or(LyricsError::Malformed)?
            }
            _ => return Err(LyricsError::Malformed),
        };
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        Ok(())
    }

    /// The second half of a surrogate pair, including the `\u` that must sit
    /// between the halves.
    fn paired_unit(&mut self) -> Result<u16, LyricsError> {
        self.expect(b'\\')?;
        self.expect(b'u')?;
        let low = self.hex4()?;
        if !(0xDC00..0xE000).contains(&low) {
            return Err(LyricsError::Malformed);
        }
        Ok(low)
    }

    fn number(&mut self) -> Result<Json, LyricsError> {
        let start = self.at;
        while matches!(
            self.peek(),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.at += 1;
        }
        // The slice is the ASCII the scan just walked over.
        let text =
            std::str::from_utf8(&self.bytes[start..self.at]).map_err(|_| LyricsError::Malformed)?;
        text.parse::<f64>().map_err(|_| LyricsError::Malformed)?;
        Ok(Json::Num)
    }

    fn literal(&mut self, word: &[u8], value: Json) -> Result<Json, LyricsError> {
        if self.bytes[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(LyricsError::Malformed)
        }
    }

    fn hex4(&mut self) -> Result<u16, LyricsError> {
        let mut value: u16 = 0;
        for _ in 0..4 {
            let digit = self.next().ok_or(LyricsError::Malformed)?;
            let nibble = match digit {
                b'0'..=b'9' => digit - b'0',
                b'a'..=b'f' => digit - b'a' + 10,
                b'A'..=b'F' => digit - b'A' + 10,
                _ => return Err(LyricsError::Malformed),
            };
            value = value * 16 + u16::from(nibble);
        }
        Ok(value)
    }

    fn expect(&mut self, byte: u8) -> Result<(), LyricsError> {
        if self.next() == Some(byte) {
            Ok(())
        } else {
            Err(LyricsError::Malformed)
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
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener};
    use std::sync::{Arc, Mutex};

    /// The canned record from the task, with the unsorted line and the rest added:
    /// LRCLIB's rows are not in order, and a parser that trusts the order drops or
    /// mis-times a third of a song.
    const CREEP: &str = concat!(
        r#"{"id":496,"trackName":"Creep","artistName":"Radiohead","albumName":"Pablo Honey","#,
        r#""duration":239.0,"instrumental":false,"#,
        r#""plainLyrics":"When you were here before\nCouldn't look you in the eye\n","#,
        r#""syncedLyrics":"[00:19.16] When you were here before\n[00:24.09]Couldn't look you in the eye\n[03:50.78] \n[00:12.00]hello again\n"}"#
    );

    const CREEP_SYNCED: &str = concat!(
        "[00:19.16] When you were here before\n",
        "[00:24.09]Couldn't look you in the eye\n",
        "[03:50.78] \n",
        "[00:12.00]hello again\n"
    );

    const NOT_FOUND: &str =
        r#"{"message":"Track not found","name":"TrackNotFound","statusCode":404}"#;

    /// Timestamps go through a decimal fraction, so equality is the wrong test.
    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.005
    }

    fn line(time: f64, text: &str) -> LyricLine {
        LyricLine {
            time_secs: time,
            text: text.to_string(),
        }
    }

    fn synced(lines: Vec<LyricLine>) -> Lyrics {
        Lyrics {
            lines,
            synced: true,
            source: "test".to_string(),
            instrumental: false,
        }
    }

    #[test]
    fn a_realistic_block_parses_into_time_order() {
        let lines = parse_synced(CREEP_SYNCED);
        assert_eq!(lines.len(), 4, "{lines:?}");
        // The unsorted line is the one at 12s: it arrived last and comes out first.
        assert!(close(lines[0].time_secs, 12.0), "{:?}", lines[0]);
        assert!(close(lines[1].time_secs, 19.16), "{:?}", lines[1]);
        assert!(close(lines[2].time_secs, 24.09), "{:?}", lines[2]);
        assert!(close(lines[3].time_secs, 230.78), "{:?}", lines[3]);
        assert_eq!(lines[0].text, "hello again");
        assert_eq!(lines[1].text, "When you were here before");
        // No space after the tag, which is in the real data.
        assert_eq!(lines[2].text, "Couldn't look you in the eye");
        // A bare tag with nothing after it is a rest, not a missing line.
        assert_eq!(lines[3].text, "");
    }

    #[test]
    fn an_empty_block_is_no_lines() {
        assert!(parse_synced("").is_empty());
        assert!(parse_synced("\n\n  \n").is_empty());
    }

    #[test]
    fn rubbish_lines_are_dropped_and_never_panic() {
        for junk in [
            "When you were here before",
            "[00.19.16] no colon inside the tag",
            "[] empty tag",
            "[00:] no seconds",
            "[:19] no minutes",
            "[abc:de] not numbers",
            "[-1:19.16] before the start",
            "!!!",
            "[00:19.16",
        ] {
            assert!(
                parse_synced(junk).is_empty(),
                "{junk:?} should parse to nothing"
            );
        }
        // One bad line among good ones costs the bad line and nothing else.
        let mixed = parse_synced("[00:01.00] one\n!!!\n[00:02.00] two");
        assert_eq!(mixed.len(), 2, "{mixed:?}");
    }

    #[test]
    fn a_tag_without_a_fraction_is_still_a_time() {
        let lines = parse_synced("[00:19] whole seconds\n[01:02.5] half");
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(close(lines[0].time_secs, 19.0));
        assert!(close(lines[1].time_secs, 62.5));
    }

    #[test]
    fn two_tags_on_one_line_become_two_lines() {
        // Never seen in the data, but the format does not forbid it, and a line
        // with a tag trak cannot explain should not be shown as-is.
        let lines = parse_synced("[00:10.00] [00:20.00] repeated chorus");
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(close(lines[0].time_secs, 10.0));
        assert!(close(lines[1].time_secs, 20.0));
        assert_eq!(lines[1].text, "repeated chorus");
    }

    #[test]
    fn plain_lyrics_have_no_time_and_no_blank_rows() {
        let lines = plain_lines("first line\n\nsecond line\n");
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines.iter().all(|l| l.time_secs.is_nan()), "{lines:?}");
        assert_eq!(lines[0].text, "first line");
        assert_eq!(lines[1].text, "second line");
        assert!(plain_lines("").is_empty());
    }

    fn creep() -> Lyrics {
        synced(parse_synced(CREEP_SYNCED))
    }

    #[test]
    fn the_current_line_is_the_last_one_that_has_started() {
        let l = creep();
        // Before, at and after each line.
        assert_eq!(index_at(&l, 0.0), None, "before the first line");
        assert_eq!(index_at(&l, 11.9), None);
        assert_eq!(index_at(&l, 12.0), Some(0), "exactly on the first line");
        assert_eq!(index_at(&l, 18.0), Some(0));
        assert_eq!(index_at(&l, 19.16), Some(1), "exactly on the second line");
        assert_eq!(index_at(&l, 24.08), Some(1));
        assert_eq!(index_at(&l, 24.09), Some(2), "exactly on the third line");
        assert_eq!(index_at(&l, 230.78), Some(3), "exactly on the rest");
        assert_eq!(index_at(&l, 10_000.0), Some(3), "past the end");
    }

    #[test]
    fn a_position_never_picks_a_line_with_no_time() {
        let plain = Lyrics {
            lines: plain_lines("one\ntwo"),
            synced: false,
            source: "test".to_string(),
            instrumental: false,
        };
        for position in [0.0, 1.0, 100.0, -1.0, f64::NAN] {
            assert_eq!(index_at(&plain, position), None, "at {position}");
        }
        // A NaN among real lines must not shadow the ones that do have a time.
        let mixed = synced(vec![
            line(0.0, "intro"),
            LyricLine {
                time_secs: f64::NAN,
                text: "untimed".to_string(),
            },
        ]);
        assert_eq!(index_at(&mixed, 5.0), Some(0));
    }

    #[test]
    fn empty_lyrics_have_no_current_line() {
        let empty = synced(Vec::new());
        assert_eq!(index_at(&empty, 0.0), None);
        assert_eq!(index_at(&empty, 100.0), None);
    }

    // A canned reply, and the test server that hands it out. The server *is* the
    // fixture: no network, no file, and nothing that can be stale.
    struct Canned {
        status: u16,
        body: String,
    }

    fn canned(status: u16, body: impl Into<String>) -> Canned {
        Canned {
            status,
            body: body.into(),
        }
    }

    struct Server {
        base: String,
        heads: Arc<Mutex<Vec<String>>>,
    }

    impl Server {
        /// The origin to pass to `fetch_from`. Loopback, so the test cannot reach
        /// the internet even by accident; a TLS handshake against a `std::net`
        /// server would need a certificate, and then the test would be about
        /// certificates.
        fn base(&self) -> &str {
            &self.base
        }

        /// How many requests arrived.
        fn requests(&self) -> usize {
            self.heads
                .lock()
                .expect("the recorder is not poisoned")
                .len()
        }

        /// The request target of the nth request: path and query, no host.
        fn path(&self, n: usize) -> String {
            self.line(n)
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string()
        }

        /// The whole first line, for the request method and version.
        fn line(&self, n: usize) -> String {
            let heads = self.heads.lock().expect("the recorder is not poisoned");
            let head = heads
                .get(n)
                .unwrap_or_else(|| panic!("no request {n}; the server saw {}", heads.len()));
            head.lines().next().unwrap_or_default().to_string()
        }

        /// The whole request head, for the User-Agent.
        fn head(&self, n: usize) -> String {
            self.heads.lock().expect("the recorder is not poisoned")[n].clone()
        }
    }

    /// Answer one connection per canned reply, in order, and record what was
    /// asked for.
    fn serve(replies: Vec<Canned>) -> Server {
        // Bound before the thread starts, so the socket is already listening: the
        // request cannot lose a race with accept(), and the test never has to
        // sleep to make that true.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let heads = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&heads);
        std::thread::spawn(move || {
            for reply in replies {
                let Ok((mut socket, _)) = listener.accept() else {
                    return;
                };
                let head = read_head(&mut socket);
                recorder
                    .lock()
                    .expect("the recorder is not poisoned")
                    .push(String::from_utf8_lossy(&head).into_owned());
                let response = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n",
                    reply.status,
                    reason(reply.status),
                    reply.body.len()
                );
                // Head and body in two writes, then a shutdown: the client must
                // never see a reply the test has not finished describing.
                let _ = socket.write_all(response.as_bytes());
                let _ = socket.write_all(reply.body.as_bytes());
                let _ = socket.flush();
                let _ = socket.shutdown(Shutdown::Both);
            }
        });
        Server {
            base: format!("http://127.0.0.1:{port}"),
            heads,
        }
    }

    /// Read to the end of the request head. One `read` is not enough: a small GET
    /// almost always arrives in one segment, and a test that depends on "almost
    /// always" fails on a busy machine.
    fn read_head(socket: &mut std::net::TcpStream) -> Vec<u8> {
        let mut head = Vec::new();
        let mut chunk = [0u8; 1024];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match socket.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => head.extend_from_slice(&chunk[..n]),
            }
            // A client that never finishes its head is not one trak's agent is.
            if head.len() > 16 * 1024 {
                break;
            }
        }
        head
    }

    fn reason(status: u16) -> &'static str {
        match status {
            200 => "OK",
            400 => "Bad Request",
            404 => "Not Found",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "Unknown",
        }
    }

    #[test]
    fn a_get_lookup_builds_and_percent_encodes_the_query() {
        let server = serve(vec![canned(200, CREEP)]);
        let lyrics = fetch_from(
            server.base(),
            "Creep",
            "Radiohead",
            Some("Pablo Honey"),
            Some(239),
        )
        .expect("a row with synced lyrics");

        assert_eq!(
            server.path(0),
            "/api/get?track_name=Creep&artist_name=Radiohead&album_name=Pablo%20Honey&duration=239"
        );
        assert_eq!(
            server.requests(),
            1,
            "one lookup should not need the search"
        );
        assert!(lyrics.synced, "{lyrics:?}");
        assert!(!lyrics.instrumental);
        assert_eq!(lyrics.lines.len(), 4, "{:?}", lyrics.lines);
        assert_eq!(lyrics.lines[1].text, "When you were here before");
        assert_eq!(lyrics.source, format!("{}/api/get", server.base()));
    }

    #[test]
    fn a_name_that_would_break_the_query_is_escaped() {
        let server = serve(vec![canned(200, CREEP)]);
        fetch_from(
            server.base(),
            "Simon & Garfunkel",
            "The 59th Street Bridge Song (Live) / 1967#1",
            None,
            None,
        )
        .expect("the row is what the server said");
        let path = server.path(0);
        assert!(
            path.contains("track_name=Simon%20%26%20Garfunkel"),
            "{path}"
        );
        assert!(
            path.contains(
                "artist_name=The%2059th%20Street%20Bridge%20Song%20%28Live%29%20%2F%201967%231"
            ),
            "{path}"
        );
        // No album and no duration means no such parameters at all, rather than
        // empty ones the server would answer with a 400.
        assert!(!path.contains("album_name"), "{path}");
        assert!(!path.contains("duration"), "{path}");
    }

    #[test]
    fn the_request_says_who_trak_is() {
        let server = serve(vec![canned(200, CREEP)]);
        fetch_from(server.base(), "Creep", "Radiohead", None, None).expect("synced");
        let head = server.head(0);
        assert!(head.contains("user-agent: trak/"), "{head}");
        assert!(head.contains("github.com/Kathir-D/trak"), "{head}");
    }

    #[test]
    fn a_duration_the_server_would_reject_is_left_out() {
        let server = serve(vec![canned(200, CREEP), canned(200, CREEP)]);
        // 0 and 4000 are both a 400 waiting to happen; 0 would also exclude every
        // row, since the server matches within about two seconds.
        for bad in [Some(0), Some(4_000)] {
            fetch_from(server.base(), "Creep", "Radiohead", None, bad).expect("synced");
        }
        for n in 0..2 {
            assert!(!server.path(n).contains("duration"), "{}", server.path(n));
        }
    }

    #[test]
    fn a_404_falls_back_to_search_and_prefers_synced() {
        let plain_row = concat!(
            r#"{"id":1,"trackName":"Creep","artistName":"Radiohead","albumName":"Pablo Honey","#,
            r#""duration":239.0,"instrumental":false,"plainLyrics":"plain words only\n","#,
            r#""syncedLyrics":null}"#
        );
        let synced_row = concat!(
            r#"{"id":2,"trackName":"Creep (Remastered)","artistName":"Radiohead","#,
            r#""albumName":"Pablo Honey","duration":238.0,"instrumental":false,"#,
            r#""plainLyrics":"When you were here before\n","#,
            r#""syncedLyrics":"[00:19.16] When you were here before\n[00:24.09] Couldn't look you in the eye"}"#
        );
        let server = serve(vec![
            canned(404, NOT_FOUND),
            canned(200, format!("[{plain_row},{synced_row}]")),
        ]);
        let lyrics = fetch_from(
            server.base(),
            "Creep",
            "Radiohead",
            Some("Pablo Honey"),
            Some(239),
        )
        .expect("the search found it");

        assert_eq!(server.requests(), 2);
        assert_eq!(
            server.path(1),
            "/api/search?q=Creep%20Radiohead",
            "the fallback searches by name"
        );
        // Row 0 is plain and comes first, but a synced line is what the TUI wants.
        assert!(lyrics.synced, "{lyrics:?}");
        assert_eq!(lyrics.lines.len(), 2, "{:?}", lyrics.lines);
        assert!(
            close(lyrics.lines[1].time_secs, 24.09),
            "{:?}",
            lyrics.lines[1]
        );
        // The source is the endpoint, without the query: a status line should not
        // carry the name of the track with it.
        assert_eq!(lyrics.source, format!("{}/api/search", server.base()));
    }

    #[test]
    fn a_search_with_nothing_in_it_is_not_found_rather_than_a_panic() {
        for body in [
            "[]",
            r#"[{"id":3,"trackName":"Creep","instrumental":true}]"#,
        ] {
            let server = serve(vec![canned(404, NOT_FOUND), canned(200, body)]);
            let e = fetch_from(server.base(), "Creep", "Radiohead", None, None)
                .expect_err("nothing in the search");
            assert!(matches!(e, LyricsError::NotFound), "{body}: {e:?}");
        }
    }

    #[test]
    fn an_empty_name_is_never_sent_to_the_server() {
        let server = serve(vec![]);
        for (title, artist) in [("", "Radiohead"), ("Creep", "  "), ("   ", "Radiohead")] {
            let e = fetch_from(server.base(), title, artist, None, None)
                .expect_err("nothing to look up");
            assert!(matches!(e, LyricsError::NotFound), "{title:?}/{artist:?}");
        }
        assert_eq!(server.requests(), 0, "an empty name is not a request");
    }

    #[test]
    fn a_busy_server_is_a_status_and_not_a_retry() {
        // 429 and 503 are how LRCLIB says "ask again later". Sleeping here would
        // hold a worker slot; the status line is the whole answer.
        for code in [429, 503] {
            let server = serve(vec![canned(code, r#"{"message":"ServerOverloaded"}"#)]);
            let e = fetch_from(server.base(), "Creep", "Radiohead", None, None)
                .expect_err("the server is busy");
            assert!(matches!(e, LyricsError::Status(c) if c == code), "{e:?}");
            assert_eq!(server.requests(), 1, "a 5xx must not trigger the search");
        }
    }

    #[test]
    fn a_body_that_is_not_json_is_malformed() {
        // A 400 comes back as plain text, so this is the shape a real mistake
        // takes: the status is checked first, and only a 200 gets parsed.
        let server = serve(vec![canned(200, "Bad Request")]);
        let e = fetch_from(server.base(), "Creep", "Radiohead", None, None)
            .expect_err("plain text is not a record");
        assert!(matches!(e, LyricsError::Malformed), "{e:?}");
    }

    #[test]
    fn a_200_with_no_lyric_text_is_malformed() {
        let server = serve(vec![canned(
            200,
            r#"{"id":4,"trackName":"Creep","instrumental":false,"plainLyrics":null,"syncedLyrics":null}"#,
        )]);
        let e = fetch_from(server.base(), "Creep", "Radiohead", None, None)
            .expect_err("a record with nothing in it");
        assert!(matches!(e, LyricsError::Malformed), "{e:?}");
    }

    #[test]
    fn an_instrumental_track_is_an_answer_with_nothing_in_it() {
        let server = serve(vec![canned(
            200,
            r#"{"id":5,"trackName":"Xylophone","instrumental":true,"duration":120.0,"plainLyrics":null,"syncedLyrics":null}"#,
        )]);
        let lyrics = fetch_from(server.base(), "Xylophone", "Artist", None, Some(120))
            .expect("instrumental is a real answer");
        assert!(lyrics.instrumental);
        assert!(lyrics.lines.is_empty());
        assert!(!lyrics.synced);
    }

    #[test]
    fn plain_only_lyrics_are_used_when_there_is_no_synced_string() {
        let server = serve(vec![canned(
            200,
            r#"{"id":6,"trackName":"Creep","instrumental":false,"plainLyrics":"first\nsecond\n","syncedLyrics":null}"#,
        )]);
        let lyrics = fetch_from(server.base(), "Creep", "Radiohead", None, None).expect("plain");
        assert!(!lyrics.synced, "{lyrics:?}");
        assert_eq!(lyrics.lines.len(), 2, "{lyrics:?}");
        assert!(lyrics.lines.iter().all(|l| l.time_secs.is_nan()));
        // Unsynced lyrics have no current line, which is why the TUI shows them
        // in full rather than scrolling them.
        assert_eq!(index_at(&lyrics, 42.0), None);
    }

    #[test]
    fn a_synced_string_that_parses_to_nothing_falls_back_to_the_plain_one() {
        let server = serve(vec![canned(
            200,
            r#"{"id":7,"trackName":"Creep","instrumental":false,"plainLyrics":"words anyway\n","syncedLyrics":"not a tag at all"}"#,
        )]);
        let lyrics = fetch_from(server.base(), "Creep", "Radiohead", None, None).expect("plain");
        assert!(!lyrics.synced);
        assert_eq!(lyrics.lines.len(), 1, "{lyrics:?}");
    }

    #[test]
    fn a_json_body_that_escapes_and_unicode_is_read() {
        let server = serve(vec![canned(
            200,
            r#"{"id":8,"trackName":"Caf\u00e9","instrumental":false,"plainLyrics":"caf\u00e9 \"quoted\"\ttabbed \u65e5\u672c\ud83c\udfb5em dash","syncedLyrics":null}"#,
        )]);
        let lyrics = fetch_from(server.base(), "Caf\u{e9}", "Artist", None, None).expect("plain");
        assert_eq!(lyrics.lines.len(), 1, "{lyrics:?}");
        assert_eq!(
            lyrics.lines[0].text,
            "caf\u{e9} \"quoted\"\ttabbed \u{65e5}\u{672c}\u{1f3b5}em dash"
        );
    }

    #[test]
    fn a_body_that_is_truncated_is_malformed_rather_than_a_panic() {
        for body in [
            "{\"syncedLyrics\":",
            "{\"a\":[1,2,",
            "{\"a\":\"\\uD83C\"}",
            "{\"a\":\"unterminated}",
            "{\"a\":1} trailing rubbish",
            "{\"a\":1.2.3}",
            "[[[[[[[[[[",
            "not json at all",
            "",
        ] {
            let e = parse_json(body).expect_err(&format!("{body:?} is not a document"));
            assert!(matches!(e, LyricsError::Malformed), "{body:?}: {e:?}");
        }
    }

    #[test]
    fn a_deep_body_is_refused_instead_of_overflowing_the_stack() {
        let body = "[".repeat(2000);
        assert!(matches!(parse_json(&body), Err(LyricsError::Malformed)));
    }

    #[test]
    fn nothing_listening_is_unreachable() {
        // Bind and drop, so the port is one the test can be sure nothing holds.
        let port = TcpListener::bind("127.0.0.1:0")
            .expect("bind")
            .local_addr()
            .expect("addr")
            .port();
        let e = fetch_from(
            &format!("http://127.0.0.1:{port}"),
            "Creep",
            "Radiohead",
            None,
            None,
        )
        .expect_err("the connection is refused");
        assert!(matches!(e, LyricsError::Unreachable), "{e:?}");
    }

    #[test]
    fn every_error_says_itself_on_one_line() {
        for e in [
            LyricsError::NotFound,
            LyricsError::Unreachable,
            LyricsError::Status(429),
            LyricsError::Status(500),
            LyricsError::Malformed,
            LyricsError::Timeout,
        ] {
            let notice = e.notice();
            assert!(!notice.is_empty(), "{e:?}");
            assert!(!notice.contains('\n'), "one line: {notice:?}");
            assert!(!notice.contains("panicked"), "no panic text: {notice:?}");
        }
    }
}
