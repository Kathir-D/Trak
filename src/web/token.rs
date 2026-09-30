//! The Spotify token file: `~/.config/trak/token.json` (TODO 7.4).
//!
//! **Why a file and not the Keychain.** TODO 1.8 measured it (`docs/KEYCHAIN.md`):
//! a keychain item is bound to the code identity of the binary that created it,
//! and `scripts/package-release.sh` re-signs on every release, so a `brew
//! upgrade` would leave every launch blocked on an authorization panel nobody can
//! dismiss. A `0600` file survives an upgrade because nothing binds it to a
//! binary. That measurement is the reason this module exists in this shape, and
//! it is why the same reasoning applies to leakage: the file is the weakest link
//! in trak's security, so it is created owner-only, rewritten atomically, and
//! the one thing that must never happen -- a token in a log line, an error
//! message or a `Debug` print -- is designed out rather than avoided.
//!
//! Four rules keep it small:
//!
//! - **A missing file is "not logged in",** which is the ordinary state for
//!   every trak that has not done the guided setup yet, and never a failure.
//! - **A file that cannot be parsed is also "not logged in", and it is left
//!   exactly where it is.** The user may want to look at it, and silently
//!   destroying a file is worse than asking for a second login.
//! - **A file other people can read is refused, not repaired on the way in.**
//!   `0600` is advisory, so trak has to be the one that checks; a token that is
//!   briefly world-readable is a token that is leaked.
//! - **Nothing here ever formats a token.** Every type that can hold one has a
//!   hand-written `Debug` that redacts, and every error is a `&'static str` or a
//!   path.
//!
//! Everything that decides anything is pure. [`Token::from_response`] and
//! [`Token::refreshed`] take the clock as an argument, [`Connection::of`] is
//! arithmetic, and the only code that touches the disk is [`TokenFile`]. Its
//! path comes in through [`Paths`], the same injectable type the config uses, so
//! no test can reach the owner's own `~/.config/trak`.

use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Write as _};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::Paths;

/// The file inside the config directory. Deliberately not part of `config.toml`:
/// the Client ID is a setting a person may type and the refresh token is a
/// credential, and one file must not be the place both live (TODO 7.4).
pub const FILE_NAME: &str = "token.json";

/// Refuse a file larger than this. A token is a few hundred bytes, so anything
/// this size is not one, and reading it would be a way to spend the owner's
/// memory on a file that happens to sit at this path.
pub const MAX_BYTES: u64 = 64 * 1024;

/// The mode a token file is created with and tightened to. The check on the way
/// in is `& 0o077`, not equality: the group and other bits are the whole risk, and
/// a read-only or owner-execute-only file is not a leak.
pub const FILE_MODE: u32 = 0o600;

/// The mode the directory is created with. The owner of `~/.config` on this
/// machine is already `0700` (`docs/KEYCHAIN.md`), so this only decides what a
/// fresh account gets.
pub const DIR_MODE: u32 = 0o700;

/// How long a refresh token lives (docs/WEB-API.md §6): "Refresh tokens issued
/// to apps registered in the Developer Dashboard have a lifetime of 6 months."
///
/// Six calendar months is 182.6 days on average, so this rounds *up*. The
/// asymmetry is deliberate: an estimate that is a little short makes trak warn
/// or ask for a login slightly early, which costs the user one extra click,
/// while an estimate that is long has trak send a refresh Spotify will refuse.
/// The server is always the authority -- [`Token::access_stale`] and
/// [`Connection::of`] only decide what the TUI *says*, never whether a refresh
/// is attempted.
pub const REFRESH_TOKEN_LIFETIME: Duration = Duration::from_secs(183 * 24 * 60 * 60);

/// How long before that trak starts saying "log in again". Spotify publishes no
/// warning and no lead time, so this is trak's own choice: two weeks is long
/// enough that a user who sees it on a Tuesday has time to do it, and short
/// enough that a half-dead connection is never what a user discovers by having
/// search fail.
pub const RECONNECT_LEAD: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// How long before the real expiry an access token is treated as stale. An
/// access token lasts one hour (docs/WEB-API.md §6) and a request takes tens of
/// milliseconds, so a small margin costs one refresh and removes the case where
/// trak sends a bearer token that expires in flight and cannot tell why the
/// answer was a 401.
pub const EXPIRY_SKEW: Duration = Duration::from_secs(30);

/// What every `Debug` print of a credential puts in its place.
const REDACTED: &str = "[redacted]";

/// The document the token endpoint returns, before it becomes a [`Token`].
///
/// The two fields trak depends on are `access_token` and `expires_in`; a
/// `refresh_token` is optional because a refresh response is not documented to
/// carry one, and [`Token::refreshed`] keeps the current one when it is absent.
/// `scope` is recorded and never acted on.
#[derive(Deserialize, PartialEq, Eq)]
pub struct TokenResponse {
    /// The bearer token for API calls.
    pub access_token: String,
    /// Only sent on the authorization exchange in practice; treated as optional
    /// everywhere.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// The access token's lifetime, one hour per docs/WEB-API.md §6.
    #[serde(default)]
    pub expires_in: Option<u64>,
    /// The scopes that were granted. Informational: trak never refuses a call
    /// because a scope is missing, it just fails soft.
    #[serde(default)]
    pub scope: Option<String>,
}

impl fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenResponse")
            .field("access_token", &REDACTED)
            .field("refresh_token", &REDACTED)
            .field("expires_in", &self.expires_in)
            .field("scope", &self.scope)
            .finish()
    }
}

/// A usable token, and the two clocks that decide how long it is one.
///
/// `authorized_at` is the field the whole reconnect design hangs on. Spotify
/// does not return a refresh token's issue time (docs/WEB-API.md §6), so it is
/// recorded locally at the moment of authorization, and it is the only evidence
/// trak will ever have of a six-month window it cannot query.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    access_token: String,
    refresh_token: String,
    /// Unix seconds, not a `SystemTime`: the file has to mean the same thing on
    /// any machine, and `SystemTime`'s representation is not portable.
    expires_at: u64,
    /// When the user authorized, which is when the six months began. Never moved
    /// by a refresh, because refreshing does not extend the refresh token's
    /// lifetime (docs/WEB-API.md §6).
    authorized_at: u64,
    /// Space-separated, as the endpoint sends it.
    #[serde(default)]
    scope: String,
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Token")
            .field("access_token", &REDACTED)
            .field("refresh_token", &REDACTED)
            .field("expires_at", &self.expires_at)
            .field("authorized_at", &self.authorized_at)
            .field("scope", &self.scope)
            .finish()
    }
}

impl Token {
    /// The token a completed login produced, stamped at the moment the user
    /// authorized -- not the moment the response came back, because the six
    /// months start at the authorization.
    ///
    /// `None` for a response with no access token or no refresh token: without
    /// the second one the next hour is the whole of what this token is worth,
    /// and a client that pretends otherwise is a client that dies silently in an
    /// hour rather than saying so.
    pub fn from_response(response: &TokenResponse, authorized_at: SystemTime) -> Option<Token> {
        Some(Token {
            access_token: non_empty(&response.access_token)?,
            refresh_token: non_empty(response.refresh_token.as_deref()?)?,
            expires_at: expiry(authorized_at, response.expires_in),
            authorized_at: unix(authorized_at),
            scope: response
                .scope
                .as_deref()
                .and_then(non_empty)
                .unwrap_or_default(),
        })
    }

    /// The token a refresh produced, on top of this one.
    ///
    /// The six-month clock is carried over untouched even when the endpoint sends
    /// a new refresh token, because the only lifetime Spotify documents is the
    /// one that starts when the user authorizes. Moving `authorized_at` forward
    /// here would be the one bug that makes the reconnect state unreachable: the
    /// window would reset every hour and never fire.
    ///
    /// A response with no access token is `None`, a response with no refresh
    /// token keeps the current one, and a response with no `expires_in` is
    /// treated as already stale -- an unknown lifetime is not one trak can plan
    /// around, and refreshing again is cheap next to sending a token that dies
    /// mid-request.
    pub fn refreshed(&self, response: &TokenResponse, now: SystemTime) -> Option<Token> {
        Some(Token {
            access_token: non_empty(&response.access_token)?,
            refresh_token: response
                .refresh_token
                .as_deref()
                .and_then(non_empty)
                .unwrap_or_else(|| self.refresh_token.clone()),
            expires_at: expiry(now, response.expires_in),
            authorized_at: self.authorized_at,
            scope: response
                .scope
                .as_deref()
                .and_then(non_empty)
                .unwrap_or_else(|| self.scope.clone()),
        })
    }

    /// The bearer token for an API call. The one accessor that hands out a
    /// secret, so it is a getter on a type that redacts itself rather than a
    /// field.
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    /// The token that buys a new access token, and the thing `Log out` and a
    /// rejected refresh throw away.
    pub fn refresh_token(&self) -> &str {
        &self.refresh_token
    }

    /// What the user authorized, space-separated. Informational only.
    pub fn scope(&self) -> &str {
        &self.scope
    }

    /// When the access token really expires.
    pub fn expires_at(&self) -> SystemTime {
        from_unix(self.expires_at)
    }

    /// When the user authorized, and so when the six-month window began.
    pub fn authorized_at(&self) -> SystemTime {
        from_unix(self.authorized_at)
    }

    /// Whether the access token is too close to its expiry to use, counting
    /// [`EXPIRY_SKEW`].
    pub fn access_stale(&self, now: SystemTime) -> bool {
        match now.checked_add(EXPIRY_SKEW) {
            Some(limit) => limit >= self.expires_at(),
            // A clock past the representable range means the comparison cannot be
            // trusted, and the safe answer is "refresh".
            None => true,
        }
    }

    /// How long the refresh token has left, or `None` once the six months are
    /// up. The moment they run out is `None` and not `Some(ZERO)`, because the
    /// window has closed at exactly that instant and a caller should not have to
    /// decide whether zero is still time. A clock behind the file's own timestamp
    /// gets the whole window back rather than a negative duration, because that is
    /// what a slow clock means and the user can see their own.
    pub fn refresh_remaining(&self, now: SystemTime) -> Option<Duration> {
        let left = self.refresh_deadline().duration_since(now).ok()?;
        if left.is_zero() { None } else { Some(left) }
    }

    /// Whether the six months are up, by the local record. A reason to send the
    /// user to the login, never a reason to skip trying.
    pub fn refresh_stale(&self, now: SystemTime) -> bool {
        self.refresh_remaining(now).is_none()
    }

    /// The moment the refresh token stops being usable, saturating rather than
    /// wrapping: `checked_add` keeps a hand-set timestamp in a test, or a
    /// corrupted one, from wrapping into the past and claiming six months are
    /// still ahead of it.
    fn refresh_deadline(&self) -> SystemTime {
        self.authorized_at()
            .checked_add(REFRESH_TOKEN_LIFETIME)
            .unwrap_or(self.authorized_at())
    }
}

/// Where a token lives, so the rest of trak can be tested without a disk and
/// the real one can be a file.
///
/// One implementation only, on purpose (`docs/KEYCHAIN.md`): a Keychain backend
/// would be a second implementation of a thing that hangs on every upgrade, and
/// the trait exists for tests and for the TUI's seam, not for a second opinion.
pub trait Store {
    /// Read the token, or say why there is not one. A file that is missing,
    /// unparseable or too loose is `Ok` with no token: none of those are
    /// failures, they are the state before a login.
    fn load(&self) -> Result<Loaded, StoreError>;

    /// Write the token, atomically, owner-only, fixing the mode of whatever is
    /// already there.
    fn save(&self, token: &Token) -> Result<(), StoreError>;

    /// Forget the token. `Log out` (TODO 7.3) and a rejected refresh token both
    /// land here. A file that is already gone is not a failure.
    fn clear(&self) -> Result<(), StoreError>;
}

/// The one real [`Store`]: `~/.config/trak/token.json`, mode `0600`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenFile {
    path: PathBuf,
}

impl TokenFile {
    /// A store for a named file, which is how the tests point it somewhere
    /// harmless.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The file SPEC §8's config directory holds: the same [`Paths`] the config
    /// uses, so `XDG_CONFIG_HOME` wins and a test can say "your home is this
    /// temporary directory" without touching the real one.
    pub fn at(paths: &Paths) -> Self {
        Self::new(paths.dir().join(FILE_NAME))
    }

    /// The file this store reads and writes.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Store for TokenFile {
    fn load(&self) -> Result<Loaded, StoreError> {
        // The mode is checked before the bytes are read, not after: a token
        // another user can read should not be pulled into this process at all,
        // and the check is one `metadata` call.
        let mode = match fs::metadata(&self.path) {
            Ok(meta) => {
                // A directory in the way of the file is a disk failure rather than
                // a bad mode: nothing was read, so there is no "not logged in" to
                // report, and a `0600` directory is not a token.
                if meta.is_dir() {
                    return Err(StoreError::Read {
                        path: self.path.clone(),
                    });
                }
                meta.permissions().mode() & 0o777
            }
            // No file is the state every trak is in until the guided setup runs.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Loaded::absent(&self.path));
            }
            Err(_) => {
                return Err(StoreError::Read {
                    path: self.path.clone(),
                });
            }
        };
        if mode & 0o077 != 0 {
            return Ok(Loaded {
                token: None,
                path: self.path.clone(),
                outcome: Outcome::TooLoose { mode },
            });
        }
        let bytes = match read_file(&self.path)? {
            Some(bytes) => bytes,
            None => return Ok(Loaded::absent(&self.path)),
        };
        match decode(&bytes) {
            Ok(token) => Ok(Loaded {
                token: Some(token),
                path: self.path.clone(),
                outcome: Outcome::Used,
            }),
            // Left on disk. A person wrote this file or a broken trak did, and
            // both are worth a look before it goes.
            Err(reason) => Ok(Loaded {
                token: None,
                path: self.path.clone(),
                outcome: Outcome::Unusable { reason },
            }),
        }
    }

    fn save(&self, token: &Token) -> Result<(), StoreError> {
        let dir = dir_of(&self.path);
        ensure_dir(dir)?;
        // Temp-then-rename, for the two reasons `config::Config::save_to` gives:
        // a half-written token is one that reads as corrupt on the next start,
        // and the rename is what publishes the mode, so a file that somebody
        // loosened to `0644` is replaced by a `0600` inode rather than repaired
        // in place.
        let temp = temp_path(&self.path);
        write_private(&temp, &encode(token)?)?;
        fs::rename(&temp, &self.path).map_err(|_| StoreError::Write {
            path: self.path.clone(),
        })
    }

    fn clear(&self) -> Result<(), StoreError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            // Logging out twice is not a failure, and neither is logging out of a
            // trak that never logged in.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(StoreError::Remove {
                path: self.path.clone(),
            }),
        }
    }
}

/// A disk failure around the token file, as opposed to a file that is not a
/// token. The two are different problems: this one is worth a line, the other
/// is worth a login.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// The file is there and cannot be read: a directory where the file should
    /// be, or a disk that has stopped answering.
    #[error("trak: could not read {path}")]
    Read { path: PathBuf },
    /// The directory that should hold the file is not there and could not be
    /// made.
    #[error("trak: could not create {path}")]
    Mkdir { path: PathBuf },
    /// The temp file, the write, the rename or the serialization failed.
    #[error("trak: could not write {path}")]
    Write { path: PathBuf },
    /// The file is there and will not go, which is the one failure that a
    /// "discard the refresh token" has to survive: the token is spent either
    /// way, and the caller is told.
    #[error("trak: could not remove {path}")]
    Remove { path: PathBuf },
}

impl StoreError {
    /// One line, for a toast. Never a stack trace, never a panic, and never any
    /// part of the file's contents.
    pub fn notice(&self) -> String {
        self.to_string()
    }
}

/// What a [`Store::load`] found, and what it did about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    /// Always usable as "not logged in" when it is `None`, which is three
    /// different situations and the same answer for all of them.
    pub token: Option<Token>,
    /// The file that was read, or would have been.
    pub path: PathBuf,
    /// What happened to it.
    pub outcome: Outcome,
}

impl Loaded {
    fn absent(path: &Path) -> Self {
        Self {
            token: None,
            path: path.to_path_buf(),
            outcome: Outcome::Absent,
        }
    }

    /// One line for a status bar, and nothing at all when there is nothing to
    /// say. Covers both the file and the six-month window, because a status bar
    /// has one line and the user has one question: is this connection going to
    /// keep working.
    pub fn notice(&self, now: SystemTime) -> Option<String> {
        match &self.outcome {
            Outcome::Absent => None,
            Outcome::Unusable { reason } => Some(format!(
                "trak: {reason}; {} is still there",
                name_of(&self.path)
            )),
            Outcome::TooLoose { mode } => Some(format!(
                "trak: {} is readable by others (mode {mode:04o}); log in again to rewrite it",
                name_of(&self.path)
            )),
            Outcome::Used => self
                .token
                .as_ref()
                .and_then(|token| window_notice(token, now)),
        }
    }
}

/// The line for a window that is closing, and nothing for one that is not. A
/// token with days left in hand is not an event; a token with an hour left is.
fn window_notice(token: &Token, now: SystemTime) -> Option<String> {
    match token.refresh_remaining(now) {
        Some(left) if left <= RECONNECT_LEAD => {
            let days = left.as_secs() / 86_400;
            Some(format!(
                "trak: the Spotify connection expires in {days} days; log in again to renew it"
            ))
        }
        Some(_) => None,
        None => Some("trak: the Spotify authorisation is over six months old; log in again".into()),
    }
}

/// What became of the file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No file. Every trak is here until the guided setup runs.
    Absent,
    /// A file that was read and used.
    Used,
    /// A file that is there and is not a token, left exactly where it is.
    Unusable {
        /// A whole sentence, and never a serde message: those quote the document,
        /// and the document is the token.
        reason: &'static str,
    },
    /// A file other people can read. Refused, and left where it is, so the owner
    /// can decide between `chmod` and logging in again.
    TooLoose {
        /// The mode it was found with, so the message can name it.
        mode: u32,
    },
}

/// What the TUI shows about the Spotify connection, derived and never stored.
///
/// A refresh token lasts six months (docs/WEB-API.md §6), so "logged in" is not
/// a state a program can hold: it is a window with an end, and the end is the
/// one moment trak must get right. A six-month-old refresh token failing has to
/// read as *reconnect Spotify*, in the TUI and in the settings screen, because
/// the alternative -- an error state, or a client that quietly stops answering
/// -- is what a user cannot act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    /// No usable token: none yet, or one that could not be read.
    LoggedOut,
    /// A token whose access token is live and whose refresh token has months
    /// left.
    Connected,
    /// Live, but the six-month window is closing.
    ExpiringSoon,
    /// Past six months by the local record, or spent. The user has to log in
    /// again; trak has already thrown the refresh token away.
    NeedsReconnect,
}

impl Connection {
    /// Derive the state from what the store found and what the clock says. Cheap
    /// and pure, so a status line can ask every time it draws.
    pub fn of(loaded: &Loaded, now: SystemTime) -> Connection {
        let Some(token) = &loaded.token else {
            return Connection::LoggedOut;
        };
        match token.refresh_remaining(now) {
            Some(left) if left <= RECONNECT_LEAD => Connection::ExpiringSoon,
            Some(_) => Connection::Connected,
            None => Connection::NeedsReconnect,
        }
    }

    /// Whether the next thing to do is a login, either because there is nothing
    /// to refresh or because what there was is spent.
    pub fn needs_relogin(self) -> bool {
        matches!(self, Connection::LoggedOut | Connection::NeedsReconnect)
    }
}

/// A response with no access token, or a scope that is only whitespace, is not
/// one; `Some` keeps the caller free of empty-string checks at every use.
fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// When an access token stops being usable, given its lifetime. A response with
/// no lifetime expires now, deliberately: see [`Token::refreshed`].
fn expiry(from: SystemTime, expires_in: Option<u64>) -> u64 {
    match expires_in {
        Some(seconds) => match from.checked_add(Duration::from_secs(seconds)) {
            Some(when) => unix(when),
            None => unix(from),
        },
        None => unix(from),
    }
}

fn unix(when: SystemTime) -> u64 {
    match when.duration_since(UNIX_EPOCH) {
        Ok(since) => since.as_secs(),
        // Before 1970: a clock that far wrong cannot be planned around, and
        // `0` is the answer that makes the token look expired rather than
        // eternal.
        Err(_) => 0,
    }
}

fn from_unix(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// The bytes on disk, or `None` for "there is no file".
fn read_file(path: &Path) -> Result<Option<Vec<u8>>, StoreError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(StoreError::Read {
                path: path.to_path_buf(),
            });
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| StoreError::Read {
            path: path.to_path_buf(),
        })?;
    Ok(Some(bytes))
}

/// A token file's bytes, or the one sentence that says why they are not a token.
///
/// Every parse failure collapses to a `&'static str`. That is the point: a
/// `serde_json::Error` names a line and a column and, in some shapes, a fragment
/// of the document, and the document here is a credential.
fn decode(bytes: &[u8]) -> Result<Token, &'static str> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err("the Spotify token file is implausibly large");
    }
    let token: Token = serde_json::from_slice(bytes)
        .map_err(|_| "the Spotify token file is not a token trak can read")?;
    // Parsing is not enough: a file that parses and carries empty tokens is one
    // that would authorize every call with nothing, which fails as a 401 the
    // user cannot connect to a cause.
    if non_empty(token.access_token()).is_none() || non_empty(token.refresh_token()).is_none() {
        return Err("the Spotify token file carries no token");
    }
    Ok(token)
}

/// The bytes to write. A `Token` is four scalars and three strings, so this
/// cannot fail in practice; the error is here because `Result` is the only shape
/// a serde call has.
fn encode(token: &Token) -> Result<Vec<u8>, StoreError> {
    serde_json::to_vec(token).map_err(|_| StoreError::Write {
        path: PathBuf::from(FILE_NAME),
    })
}

/// The directory a save has to create, owner-only if trak had to make it.
///
/// The mode is set after the fact rather than at creation because `create_dir_all`
/// has no mode argument, and the window it opens is harmless: the directory holds
/// no token until the rename that follows.
fn ensure_dir(dir: &Path) -> Result<(), StoreError> {
    if dir.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(dir).map_err(|_| StoreError::Mkdir {
        path: dir.to_path_buf(),
    })?;
    fs::set_permissions(dir, fs::Permissions::from_mode(DIR_MODE)).map_err(|_| StoreError::Mkdir {
        path: dir.to_path_buf(),
    })
}

/// Write `bytes` to `path`, owner-only, flushed before the caller renames it.
///
/// The mode is set at creation *and* again after the write, for the two reasons
/// `config::Config::save_to` gives: the creation mode is masked by the umask, and
/// a temp file left behind by a killed trak is a file that already exists with
/// whatever mode it was last written with. A token is not a setting, so this one
/// is not negotiable.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let wrote = |_| StoreError::Write {
        path: path.to_path_buf(),
    };
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(FILE_MODE)
        .open(path)
        .map_err(wrote)?;
    file.write_all(bytes).map_err(wrote)?;
    fs::set_permissions(path, fs::Permissions::from_mode(FILE_MODE)).map_err(wrote)?;
    // Flushed before the rename, or the rename can land with the contents still
    // in the buffer: a zero-length token, which reads as no token at all.
    file.sync_all().map_err(wrote)
}

/// The directory a file's parent is, with the empty parent that a bare file name
/// has resolved to the current directory.
fn dir_of(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// The file's own name, for a message about it.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| FILE_NAME.to_string())
}

/// A sibling of the file being written, so the rename publishes it atomically
/// and the temp file is on the same filesystem as its destination.
fn temp_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(".{}.tmp", name_of(path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary directory that removes itself. No tempfile dependency for one
    /// struct, and the name is per test because cargo runs them in threads of one
    /// process.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "trak-token-test-{tag}-{}-{:?}",
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

    /// A config directory that removes itself, and the [`TokenFile`] in it. This
    /// is the whole reason the path is injectable: nothing here can reach the
    /// owner's own `~/.config/trak`. Note that the store's own path is used
    /// throughout rather than `Paths::file`, which is the *config's* file.
    fn sandbox(tag: &str) -> (TempDir, TokenFile) {
        let home = TempDir::new(tag);
        let store = TokenFile::at(&Paths::from_vars(None, Some(home.path().to_path_buf())));
        (home, store)
    }

    fn dir_of_store(store: &TokenFile) -> PathBuf {
        store.path().parent().expect("a dir").to_path_buf()
    }

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    /// Write a token file the way trak writes one, so a test that means to be
    /// about the *contents* is not also about the mode: `fs::write` creates
    /// `0644`, which trak refuses to read, on purpose.
    fn write_token_file(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("token dir");
        }
        fs::write(path, body).expect("write");
        fs::set_permissions(path, fs::Permissions::from_mode(FILE_MODE)).expect("owner only");
    }

    /// A moment in time well clear of the epoch, so nothing here depends on what
    /// the machine's clock says.
    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn days(n: u64) -> Duration {
        Duration::from_secs(n * 24 * 60 * 60)
    }

    /// A token as the endpoint would send it, with the parts a test wants to vary
    /// left at Spotify's real values.
    fn response() -> TokenResponse {
        TokenResponse {
            access_token: "access-abc".into(),
            refresh_token: Some("refresh-xyz".into()),
            // One hour, per docs/WEB-API.md §6.
            expires_in: Some(3600),
            scope: Some("user-library-read playlist-read-private".into()),
        }
    }

    /// A token the tests own, authorized at `at` and good until an hour later.
    fn token_at(at: SystemTime) -> Token {
        Token::from_response(&response(), at).expect("a token from a good response")
    }

    #[test]
    fn a_token_round_trips_through_the_file() {
        let (_home, store) = sandbox("round-trip");
        let token = token_at(now());
        store.save(&token).expect("save");
        let loaded = store.load().expect("load");
        assert_eq!(loaded.token, Some(token.clone()));
        assert_eq!(loaded.outcome, Outcome::Used);
        assert!(loaded.notice(now()).is_none(), "a good token says nothing");
        // And the two clocks survive the file, which is the part the reconnect
        // state depends on.
        let read_back = loaded.token.as_ref().expect("token");
        assert_eq!(read_back.authorized_at(), token.authorized_at());
        assert_eq!(read_back.expires_at(), token.expires_at());
    }

    #[test]
    fn a_saved_token_file_is_owner_only() {
        let (_home, store) = sandbox("mode");
        store.save(&token_at(now())).expect("save");
        assert_eq!(mode_of(store.path()), FILE_MODE);
    }

    #[test]
    fn a_first_save_makes_its_directory_owner_only() {
        let (_home, store) = sandbox("mkdir");
        let dir = dir_of_store(&store);
        assert!(!dir.exists());
        store.save(&token_at(now())).expect("save");
        assert!(dir.is_dir());
        assert_eq!(mode_of(&dir), DIR_MODE);
        assert_eq!(store.path(), dir.join(FILE_NAME));
    }

    /// The `0644` case: a file somebody loosened, a synced token, or a `cp` that
    /// kept the source's umask. Reading it would be a leak, and the save that
    /// replaces it has to leave a `0600` inode behind.
    #[test]
    fn a_save_tightens_a_file_that_was_left_readable() {
        let (_home, store) = sandbox("loose");
        write_token_file(store.path(), "not a token");
        fs::set_permissions(store.path(), fs::Permissions::from_mode(0o644)).expect("loosen");
        store.save(&token_at(now())).expect("save");
        assert_eq!(
            mode_of(store.path()),
            FILE_MODE,
            "the mode comes from the temp file, not from what was there"
        );
        assert_eq!(store.load().expect("load").token, Some(token_at(now())));
        let dir = dir_of_store(&store);
        let names: Vec<String> = fs::read_dir(&dir)
            .expect("read dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![FILE_NAME.to_string()], "no temp file survives");
    }

    #[test]
    fn a_file_other_people_can_read_is_refused_and_kept() {
        let (_home, store) = sandbox("too-loose");
        store.save(&token_at(now())).expect("save");
        fs::set_permissions(store.path(), fs::Permissions::from_mode(0o604)).expect("loosen");
        let loaded = store.load().expect("load");
        assert_eq!(loaded.token, None, "it is not read at all");
        assert_eq!(loaded.outcome, Outcome::TooLoose { mode: 0o604 });
        assert!(store.path().exists(), "and it is not deleted either");
        let notice = loaded.notice(now()).expect("something to say");
        assert!(notice.contains("readable by others"), "{notice:?}");
        assert!(notice.contains("0604"), "{notice:?}");
        // The one way out needs no `chmod`: logging in again rewrites the file.
        store.save(&token_at(now())).expect("save");
        assert_eq!(store.load().expect("load").token, Some(token_at(now())));
    }

    /// `0600` is what trak writes, and the group and other bits are the only ones
    /// that are a leak -- so a file somebody deliberately tightened, or one that
    /// arrived with an owner-execute bit from a umask oddity, is read rather than
    /// refused. Refusing it would lock a user out of a file they own.
    #[test]
    fn a_file_no_one_else_can_read_is_accepted_whatever_its_owner_bits() {
        let (_home, store) = sandbox("owner-bits");
        store.save(&token_at(now())).expect("save");
        for mode in [0o400, 0o700] {
            fs::set_permissions(store.path(), fs::Permissions::from_mode(mode)).expect("mode");
            assert_eq!(
                store.load().expect("load").token,
                Some(token_at(now())),
                "mode {mode:04o} is not a leak"
            );
        }
    }

    #[test]
    fn no_token_file_is_not_logged_in_and_says_nothing() {
        let (_home, store) = sandbox("absent");
        let loaded = store.load().expect("load");
        assert_eq!(loaded.token, None);
        assert_eq!(loaded.outcome, Outcome::Absent);
        assert_eq!(Connection::of(&loaded, now()), Connection::LoggedOut);
        assert!(Connection::LoggedOut.needs_relogin());
        assert!(loaded.notice(now()).is_none(), "no file is not an event");
    }

    /// The rule from `config.rs`, in the other direction: a broken file is moved
    /// aside, a *token* is not. A person who wants to see what went wrong can,
    /// and a second login does not cost them anything.
    #[test]
    fn a_corrupt_token_file_is_not_logged_in_and_is_left_on_disk() {
        let (_home, store) = sandbox("corrupt");
        for body in ["", "{}", "{", &"x".repeat(MAX_BYTES as usize + 1)] {
            write_token_file(store.path(), body);
            let loaded = store.load().expect("load");
            assert_eq!(loaded.token, None, "{body:?} is not a token");
            assert!(matches!(loaded.outcome, Outcome::Unusable { .. }));
            assert_eq!(fs::read_to_string(store.path()).expect("still there"), body);
            let notice = loaded.notice(now()).expect("something to say");
            assert!(notice.contains("still there"), "{notice:?}");
            assert!(notice.contains(FILE_NAME), "{notice:?}");
        }
    }

    #[test]
    fn a_token_file_that_parses_but_carries_nothing_is_not_a_token() {
        let (_home, store) = sandbox("empty-tokens");
        write_token_file(
            store.path(),
            r#"{"access_token":"","refresh_token":"  ","expires_at":0,"authorized_at":0}"#,
        );
        let loaded = store.load().expect("load");
        assert_eq!(loaded.token, None);
        let notice = loaded.notice(now()).expect("something to say");
        assert!(notice.contains("no token"), "{notice:?}");
    }

    /// The house rule from `config.rs`, applied to a file trak also writes: a key
    /// from another version is stepped over, and one that has no default is a
    /// file to refuse rather than a value to invent.
    #[test]
    fn a_token_file_from_another_version_is_still_readable() {
        let (_home, store) = sandbox("other-version");
        let stamp = unix(now());
        write_token_file(
            store.path(),
            &format!(
                r#"{{"access_token":"a","refresh_token":"r","expires_at":{},"authorized_at":{stamp},"future":{{"nested":[1]}}}}"#,
                stamp + 3600
            ),
        );
        let loaded = store.load().expect("load");
        let token = loaded.token.expect("no scope is not a problem");
        assert_eq!(token.access_token(), "a");
        assert_eq!(token.scope(), "");
        assert_eq!(token.authorized_at(), now());
    }

    #[test]
    fn the_config_home_is_respected_and_home_is_the_fallback() {
        let home = PathBuf::from("/home/someone");
        let xdg = PathBuf::from("/xdg");
        assert_eq!(
            TokenFile::at(&Paths::from_vars(Some(xdg.clone()), Some(home.clone()))).path(),
            xdg.join("trak").join(FILE_NAME)
        );
        assert_eq!(
            TokenFile::at(&Paths::from_vars(None, Some(home))).path(),
            PathBuf::from("/home/someone/.config/trak").join(FILE_NAME)
        );
    }

    /// The read is bounded before it happens, not after, so a file that is not a
    /// token cannot be used to spend memory.
    #[test]
    fn a_huge_token_file_is_refused_without_being_read() {
        let (_home, store) = sandbox("huge");
        let body = format!(r#"{{"access_token":"{}"}}"#, "a".repeat(MAX_BYTES as usize));
        write_token_file(store.path(), &body);
        let loaded = store.load().expect("load");
        assert_eq!(loaded.token, None);
        let notice = loaded.notice(now()).expect("something to say");
        assert!(notice.contains("implausibly large"), "{notice:?}");
    }

    #[test]
    fn a_cleared_token_file_is_forgotten_and_clearing_twice_is_fine() {
        let (_home, store) = sandbox("clear");
        store.clear().expect("clearing nothing is not a failure");
        store.save(&token_at(now())).expect("save");
        store.clear().expect("clear");
        assert!(!store.path().exists());
        assert_eq!(store.load().expect("load").outcome, Outcome::Absent);
    }

    /// The leakage rule, asserted rather than promised: a token is a secret, and a
    /// `Debug` print is how secrets get into logs.
    #[test]
    fn debug_never_shows_a_token() {
        let token = token_at(now());
        let debug = format!("{token:?}");
        for secret in [token.access_token(), token.refresh_token()] {
            assert!(!debug.contains(secret), "{debug:?} leaked a token");
        }
        // The file has to contain it, or nothing works at all.
        let bytes = encode(&token).expect("encode");
        assert!(String::from_utf8_lossy(&bytes).contains("access-abc"));

        let response = response();
        let debug = format!("{response:?}");
        assert!(!debug.contains("access-abc"), "{debug:?} leaked a token");
        assert!(!debug.contains("refresh-xyz"), "{debug:?} leaked a token");
    }

    #[test]
    fn a_token_that_parses_is_never_named_in_an_error() {
        let (_home, store) = sandbox("no-leak-in-errors");
        // Truncated in the middle of a token: the parse fails on bytes that are
        // half a credential.
        write_token_file(store.path(), r#"{"access_token":"access-abc","refresh"#);
        let loaded = store.load().expect("load");
        let notice = loaded.notice(now()).expect("something to say");
        assert!(!notice.contains("access-abc"), "{notice:?}");
        // And a disk failure names the path, which is the only thing it may name.
        let error = StoreError::Write {
            path: store.path().to_path_buf(),
        };
        assert!(!error.notice().contains("access-abc"));
        assert!(error.notice().contains(FILE_NAME));
    }

    /// The six-month window, at both edges and in between: the whole reconnect
    /// design is these comparisons, so they are pinned rather than described.
    #[test]
    fn the_six_month_window_decides_when_a_reconnect_is_due() {
        let authorized = now();
        let token = token_at(authorized);
        let inside = authorized + REFRESH_TOKEN_LIFETIME - days(1);
        let last_second = authorized + REFRESH_TOKEN_LIFETIME - Duration::from_secs(1);
        let at_the_end = authorized + REFRESH_TOKEN_LIFETIME;

        assert!(!token.refresh_stale(inside));
        assert_eq!(token.refresh_remaining(inside), Some(days(1)));
        assert!(!token.refresh_stale(last_second));
        assert!(token.refresh_stale(at_the_end), "zero left is spent");
        assert_eq!(token.refresh_remaining(at_the_end), None);
        // Not negative: a clock behind the file's own timestamp is a slow clock,
        // and the user can see their own.
        assert_eq!(
            token.refresh_remaining(authorized - days(30)),
            Some(REFRESH_TOKEN_LIFETIME + days(30))
        );
    }

    #[test]
    fn an_access_token_is_stale_before_it_actually_expires() {
        let authorized = now();
        let token = token_at(authorized);
        // One hour, per Spotify, and the skew comes off the front of it: at the
        // last moment inside the skew the token is already stale, which is the
        // point of having one.
        let last_fresh = authorized + Duration::from_secs(3600 - EXPIRY_SKEW.as_secs() - 1);
        let stale = authorized + Duration::from_secs(3600 - EXPIRY_SKEW.as_secs());
        assert!(!token.access_stale(last_fresh));
        assert!(token.access_stale(stale));
        assert_eq!(token.expires_at(), authorized + Duration::from_secs(3600));
    }

    /// The four states the TUI can show, and the line that goes with each.
    #[test]
    fn a_connection_reports_expiring_before_it_reports_spent() {
        let (_home, store) = sandbox("connection");
        let authorized = now();

        assert_eq!(
            Connection::of(&store.load().expect("load"), authorized),
            Connection::LoggedOut
        );

        store.save(&token_at(authorized)).expect("save");
        let loaded = store.load().expect("load");
        assert_eq!(Connection::of(&loaded, authorized), Connection::Connected);
        assert!(!Connection::Connected.needs_relogin());
        assert!(loaded.notice(authorized).is_none());

        // Inside the lead, but nowhere near the end: the one state that is a
        // warning rather than an event.
        let soon = authorized + REFRESH_TOKEN_LIFETIME - days(3);
        let loaded = store.load().expect("load");
        assert_eq!(Connection::of(&loaded, soon), Connection::ExpiringSoon);
        assert!(!Connection::ExpiringSoon.needs_relogin());
        let notice = loaded.notice(soon).expect("something to say");
        assert!(notice.contains("3 days"), "{notice:?}");
        assert!(notice.contains("log in again"), "{notice:?}");

        let spent = authorized + REFRESH_TOKEN_LIFETIME + days(1);
        let loaded = store.load().expect("load");
        assert_eq!(Connection::of(&loaded, spent), Connection::NeedsReconnect);
        assert!(Connection::NeedsReconnect.needs_relogin());
        let notice = loaded.notice(spent).expect("something to say");
        assert!(notice.contains("six months"), "{notice:?}");
    }

    /// A refresh must not move the six-month clock, even when the endpoint hands
    /// back a new refresh token. Getting this wrong makes the reconnect state
    /// unreachable, which is why it is its own test rather than part of a round
    /// trip.
    #[test]
    fn a_refresh_never_extends_the_refresh_token() {
        let authorized = now();
        let token = token_at(authorized);
        let mut response = response();
        response.refresh_token = Some("refresh-new".into());
        let refreshed = token
            .refreshed(&response, authorized + Duration::from_secs(7200))
            .expect("a token from a good response");
        assert_eq!(refreshed.access_token(), "access-abc");
        assert_eq!(
            refreshed.refresh_token(),
            "refresh-new",
            "a new refresh token is used"
        );
        assert_eq!(
            refreshed.authorized_at(),
            authorized,
            "the clock does not move"
        );
        assert_eq!(
            refreshed.expires_at(),
            authorized + Duration::from_secs(7200 + 3600),
            "and the new access token gets its own hour"
        );
    }

    /// A refresh response is not documented to carry a refresh token, so the
    /// current one is kept, and a response with no lifetime is one trak will not
    /// plan around.
    #[test]
    fn a_refresh_keeps_the_current_refresh_token_when_none_arrives() {
        let authorized = now();
        let token = token_at(authorized);
        let response = TokenResponse {
            access_token: "access-new".into(),
            refresh_token: None,
            expires_in: None,
            scope: None,
        };
        let refreshed = token
            .refreshed(&response, authorized + Duration::from_secs(7200))
            .expect("a token from a response with an access token");
        assert_eq!(refreshed.refresh_token(), "refresh-xyz");
        assert_eq!(refreshed.scope(), token.scope());
        assert!(refreshed.access_stale(authorized + Duration::from_secs(7200)));
    }

    #[test]
    fn a_response_with_no_access_token_is_not_a_token() {
        let at = now();
        let empty = TokenResponse {
            access_token: "   ".into(),
            refresh_token: Some("r".into()),
            expires_in: Some(3600),
            scope: None,
        };
        assert_eq!(Token::from_response(&empty, at), None);
        let mut no_refresh = response();
        no_refresh.refresh_token = None;
        assert_eq!(
            Token::from_response(&no_refresh, at),
            None,
            "a login that cannot be refreshed is not a login"
        );
    }

    /// A directory where the file should be is a disk failure, not an unusable
    /// token: nothing was read, so there is no "not logged in" to report.
    #[test]
    fn a_directory_where_the_token_file_should_be_is_a_disk_failure() {
        let (_home, store) = sandbox("isdir");
        fs::create_dir_all(store.path()).expect("dir");
        assert!(matches!(store.load(), Err(StoreError::Read { .. })));
    }

    /// One line, no newline, and nothing but a path. Every variant, because a
    /// variant nobody constructs in a test is a variant nobody has read.
    #[test]
    fn every_store_error_is_one_line_and_names_only_a_path() {
        let path = PathBuf::from("/tmp/trak/token.json");
        let errors = [
            StoreError::Read { path: path.clone() },
            StoreError::Mkdir { path: path.clone() },
            StoreError::Write { path: path.clone() },
            StoreError::Remove { path },
        ];
        for error in &errors {
            let notice = error.notice();
            assert!(!notice.contains('\n'), "{notice:?}");
            assert!(notice.starts_with("trak: "), "{notice:?}");
            assert!(notice.contains("token.json"), "{notice:?}");
            assert_eq!(notice, error.to_string(), "the notice is the message");
        }
    }
}
