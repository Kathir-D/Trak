//! The Spotify login: Authorization Code + PKCE over a loopback listener
//! (TODO 7.2, SPEC §6).
//!
//! Four constraints come from `docs/WEB-API.md` and every one of them is a
//! decision rather than a default:
//!
//! - **The registered redirect URI is `http://127.0.0.1`, no port and no path**
//!   (§1). Spotify allows a dynamic port *only* for a loopback IP literal
//!   registered without one, so every login binds an ephemeral port and sends
//!   the matching `redirect_uri`. A fixed port would mean trak fails to log in
//!   whenever it is taken, which on a laptop is an ordinary Tuesday.
//! - **`localhost` is banned** (§1, quoted: "localhost is not allowed as redirect
//!   URI"). Not a style preference: the rule is enforced. Every URL and every
//!   request body this module builds is asserted to not contain it.
//! - **The client is public.** PKCE is "the recommended authorization flow if
//!   you're implementing authorization in an application where the client secret
//!   can't be safely stored" (§5), so there is no secret anywhere in this
//!   module and `client_id` goes in the body of both token requests.
//! - **A refresh token lasts six months** (§6) and refreshing "does not extend
//!   the refresh token's lifetime". [`Session`] is the half of login that never
//!   opens a browser: it refreshes a stale access token, and a refresh token
//!   that is spent reads as [`Auth::Reconnect`] -- a state the TUI shows -- and
//!   not as an error to sit in.
//!
//! Everything that touches the outside world is a trait: [`Browser`], because a
//! test must not open anything, and [`TokenEndpoint`], because the tests' done-when
//! is a mock token endpoint. The loopback listener is *not* behind a trait: it
//! is a real `TcpListener` on `127.0.0.1:0` in the tests, so the redirect path
//! that ships is the one that is tested.

use std::fmt;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

use base64::Engine as _;
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::web::token::{Store, StoreError, Token, TokenResponse};

/// The redirect URI to register in the Spotify dashboard, verbatim
/// (docs/WEB-API.md §1): an explicit loopback IP literal, no port, no path.
///
/// The dynamic port trak binds at login time is *added* to this, and only this
/// form may be, because "the only exception [to exact match] is for loopback IP
/// literals, which can dynamically be assigned ports". The dashboard's
/// acceptance of a no-path form is still unconfirmed; TODO 7.3 owns that check
/// and the fixed-port fallback.
pub const REGISTERED_REDIRECT_URI: &str = "http://127.0.0.1";

/// Where the browser is sent to authorize.
///
/// **Not recorded in `docs/WEB-API.md`** -- that doc researched the rules this
/// module obeys and cites them, but does not list the hosts. The rule it does
/// record is that the implicit grant is deprecated and PKCE is the recommended
/// flow, so the flow is right even though the hostname is unverified. It is
/// injectable for that reason as much as for testing: the first real login
/// confirms it ([owner], TODO 7.2), and a wrong host is a constant to change
/// rather than a protocol to redesign.
pub const AUTHORIZE_ENDPOINT: &str = "https://accounts.spotify.com/authorize";

/// The token endpoint both PKCE requests post to. Same provenance as
/// [`AUTHORIZE_ENDPOINT`]: not in the researched doc, and a constant.
pub const TOKEN_ENDPOINT: &str = "https://accounts.spotify.com/api/token";

/// Give up on the redirect after this.
///
/// The user has to log in, pick an account, consent and possibly do two-factor in
/// a browser, so it is measured in minutes and not seconds. Five is a judgement:
/// long enough for a human with a password manager, short enough that a closed
/// tab or a cancelled browser does not leave the TUI waiting. Nothing waits on
/// this that the user cannot walk away from -- the listener is a socket, and a
/// timeout closes it.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// How long a single token request may take. A hung HTTPS request must not hold a
/// worker for the rest of the session.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Refuse a token response larger than this. A token is a few hundred bytes.
const MAX_BYTES: u64 = 64 * 1024;

/// How often an idle loopback listener looks for the redirect. 25 ms is well
/// under a frame and costs nothing next to the browser it is waiting for.
const POLL: Duration = Duration::from_millis(25);

/// How long one accepted connection may take to say something. A socket that
/// connects and then stalls is not a browser, and this is the difference between
/// a prompt error and a wait that ends when the outer timeout does.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// How much of a request line is read before it is treated as something other
/// than a redirect. Chrome's GET is a few hundred bytes.
const MAX_HEAD: usize = 8 * 1024;

/// Say the same for every version, the way `lyrics.rs` does.
const USER_AGENT: &str = concat!(
    "trak/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/Kathir-D/trak)"
);

/// Bytes of randomness behind a verifier or a `state`.
///
/// PKCE fixes the *shape* of a verifier, not its source: RFC 7636 §4.1 allows
/// 43 to 128 characters from `[A-Za-z0-9-._~]`, and 32 random bytes is 43
/// unpadded base64url characters, which is inside that set by construction.
const ENTROPY_BYTES: usize = 32;

/// The verifier, the challenge derived from it, and the `state` for one attempt.
///
/// All three are per-login. The verifier is a secret the token endpoint checks
/// the challenge against; `state` is what ties the redirect that comes back to
/// the request that went out, so a code delivered to trak's port by anything
/// else is not exchanged.
pub struct Pkce {
    verifier: String,
    challenge: String,
    state: String,
}

impl Pkce {
    /// A fresh pair. Every call is a different login, which is what makes a
    /// reused verifier (and the code it would accept) impossible.
    pub fn generate() -> Self {
        let verifier = base64url(&random(ENTROPY_BYTES));
        Self {
            challenge: challenge_for(&verifier),
            state: base64url(&random(ENTROPY_BYTES)),
            verifier,
        }
    }

    /// The secret, sent only to the token endpoint.
    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    /// The S256 challenge, sent to the browser.
    pub fn challenge(&self) -> &str {
        &self.challenge
    }

    /// The value the redirect must echo.
    pub fn state(&self) -> &str {
        &self.state
    }
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce")
            .field("verifier", &REDACTED)
            .field("challenge", &self.challenge)
            .field("state", &self.state)
            .finish()
    }
}

/// The challenge for `verifier`: `BASE64URL(SHA256(verifier))`, unpadded.
///
/// `code_challenge_method=S256` is what docs/WEB-API.md §5 records, and the
/// transformation is fixed by the method: SHA-256 of the verifier's ASCII bytes,
/// base64url with no padding.
pub fn challenge_for(verifier: &str) -> String {
    base64url(Sha256::digest(verifier.as_bytes()).as_slice())
}

/// base64url with no padding, for everything that travels as a query value or a
/// form field.
///
/// Unpadded because `=` in a query value is a parsing question nobody should have
/// to answer, and URL-safe alphabet because a standard-alphabet `+` arrives at
/// the other end as a space often enough to break a verifier.
pub fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The URL the browser is sent to. Pure, so the `localhost` rule can be asserted
/// without opening anything.
///
/// `state` and the challenge are per-login values with no `&` or `=` in them, but
/// everything is still escaped: `redirect_uri` genuinely contains `:` and `/`,
/// and a scope list contains spaces.
pub fn authorize_url(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
    scopes: &[&str],
) -> String {
    let mut url = format!(
        "{AUTHORIZE_ENDPOINT}?response_type=code&client_id={}&redirect_uri={}&state={}\
         &code_challenge={}&code_challenge_method=S256",
        percent_encode(client_id),
        percent_encode(redirect_uri),
        percent_encode(state),
        percent_encode(challenge),
    );
    if !scopes.is_empty() {
        url.push_str("&scope=");
        url.push_str(&percent_encode(&scopes.join(" ")));
    }
    url
}

/// The authorization code trak got, and what it is trading it for.
///
/// A plain holder rather than a builder: the flow is `authorize_url` then
/// `exchange`, and a struct that remembered the PKCE pair and the client ID would
/// be a second way to spell the same three strings.
pub struct CodeExchange<'a> {
    /// The one-time code out of the redirect.
    pub code: &'a str,
    /// The redirect URI the authorization request carried, which the token
    /// request has to repeat exactly.
    pub redirect_uri: &'a str,
    /// The PKCE verifier, which is what proves this trak is the one that started
    /// the authorization.
    pub code_verifier: &'a str,
    /// The Client ID from `config.toml`. An identifier rather than a secret, and
    /// it goes in the body because docs/WEB-API.md §5 says PKCE clients are public
    /// and send it this way -- which is the whole reason there is no client secret
    /// anywhere in trak.
    pub client_id: &'a str,
}

impl fmt::Debug for CodeExchange<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodeExchange")
            .field("code", &REDACTED)
            .field("redirect_uri", &self.redirect_uri)
            .field("code_verifier", &REDACTED)
            .field("client_id", &self.client_id)
            .finish()
    }
}

/// A refresh, and the token being spent.
pub struct RefreshRequest<'a> {
    /// The refresh token. Reusable only once per exchange, and worthless after
    /// six months (docs/WEB-API.md §6).
    pub refresh_token: &'a str,
    /// The Client ID, as in [`CodeExchange`].
    pub client_id: &'a str,
}

impl fmt::Debug for RefreshRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RefreshRequest")
            .field("refresh_token", &REDACTED)
            .field("client_id", &self.client_id)
            .finish()
    }
}

/// Spotify's token endpoint, as trak needs it.
///
/// Two methods rather than one because the two requests fail differently, and the
/// difference is the whole of the reconnect design: a grant the endpoint will not
/// accept is spent and must be thrown away, while a transport failure or a 5xx is
/// the network's problem and must leave the stored token alone.
pub trait TokenEndpoint {
    /// Trade an authorization code for a token.
    fn exchange(&self, request: &CodeExchange<'_>) -> Result<TokenResponse, AuthError>;

    /// Trade a refresh token for a new access token.
    fn refresh(&self, request: &RefreshRequest<'_>) -> Result<TokenResponse, AuthError>;
}

/// The real [`TokenEndpoint`].
///
/// Blocking on purpose, like every other call trak makes: this runs on a worker
/// thread behind a trait, and an async client would drag a runtime into a TUI
/// that has none (`Cargo.toml`).
pub struct SpotifyEndpoint {
    url: String,
    agent: ureq::Agent,
}

impl Default for SpotifyEndpoint {
    fn default() -> Self {
        Self::at(TOKEN_ENDPOINT)
    }
}

impl SpotifyEndpoint {
    /// An endpoint at a named URL. The tests use this to name their loopback mock,
    /// so the form encoding, the HTTP round trip and the response parsing are all
    /// covered without a network; the real host is the same code with a different
    /// string.
    pub fn at(url: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            // Spotify's own errors are answers, not exceptions: a 400 is how it
            // says the refresh token is gone, and that is a state, not a fault.
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .build()
            .new_agent();
        Self {
            url: url.to_string(),
            agent,
        }
    }
}

impl TokenEndpoint for SpotifyEndpoint {
    fn exchange(&self, request: &CodeExchange<'_>) -> Result<TokenResponse, AuthError> {
        let form: Vec<(&str, &str)> = vec![
            ("grant_type", "authorization_code"),
            ("code", request.code),
            ("redirect_uri", request.redirect_uri),
            ("client_id", request.client_id),
            ("code_verifier", request.code_verifier),
        ];
        match self.post(&form)? {
            (200..=299, body) => parse_response(&body),
            // Any other status is a grant Spotify will not accept -- a Client ID
            // that does not match the redirect URI, a code already used, consent
            // refused. The body is never quoted into the error: a token endpoint
            // that echoes a request is echoing a credential.
            _ => Err(AuthError::Refused),
        }
    }

    fn refresh(&self, request: &RefreshRequest<'_>) -> Result<TokenResponse, AuthError> {
        let form: Vec<(&str, &str)> = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", request.refresh_token),
            ("client_id", request.client_id),
        ];
        match self.post(&form)? {
            (200..=299, body) => parse_response(&body),
            // An invalid or expired refresh token: docs/WEB-API.md §6 says to
            // discard it and reauthorize. The docs do not publish a status code
            // for this one case, so *any* 4xx is treated that way -- the two ends
            // are the same outcome for the user either way. A 5xx and a transport
            // failure are not the token's fault and are left alone.
            (400..=499, _) => Err(AuthError::RefreshRejected),
            _ => Err(AuthError::Unreachable),
        }
    }
}

impl SpotifyEndpoint {
    fn post(&self, form: &[(&str, &str)]) -> Result<(u16, String), AuthError> {
        let mut response = self
            .agent
            .post(&self.url)
            .send_form(form.iter().copied())
            .map_err(transport)?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BYTES)
            .read_to_string()
            .map_err(transport)?;
        Ok((status, body))
    }
}

/// One token response, as JSON. A malformed one is `Malformed` and nothing else:
/// the body may quote the request that produced it.
fn parse_response(body: &str) -> Result<TokenResponse, AuthError> {
    if body.len() as u64 > MAX_BYTES {
        return Err(AuthError::Malformed);
    }
    serde_json::from_str(body).map_err(|_| AuthError::Malformed)
}

fn transport(e: ureq::Error) -> AuthError {
    match e {
        ureq::Error::Timeout(_) => AuthError::Unreachable,
        // `http_status_as_error(false)` means this arm is unreachable, and it is
        // kept so a status that becomes an error again is not mistaken for a
        // transport fault.
        ureq::Error::StatusCode(_) => AuthError::Unreachable,
        _ => AuthError::Unreachable,
    }
}

/// Sending the user to a browser.
///
/// A trait, and not a function, because trak's own tests must not open anything:
/// a test that launches Safari is a test that fails on an unattended machine and
/// hijacks the owner's screen at the same time.
pub trait Browser {
    /// Open `url`. The URL is an `https:` one, always.
    fn open(&self, url: &str) -> Result<(), AuthError>;
}

/// The real [`Browser`]: `/usr/bin/open`, the same call TODO 7.3's guided setup
/// makes to reach the dashboard.
///
/// This is the one place in trak that can start another application, so it is
/// worth being exact about what it cannot become: trak only ever hands it an
/// `https://accounts.spotify.com/...` URL, never `spotify:` and never a path. A
/// `spotify:` URL would be COMPAT rule 2's forbidden side effect, and the check
/// that keeps that from being a refactor away is [`Browser::open`] taking the URL
/// a caller built rather than assembling one itself.
pub struct MacBrowser;

impl Browser for MacBrowser {
    fn open(&self, url: &str) -> Result<(), AuthError> {
        let opened = Command::new("open")
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .map_err(|_| AuthError::Browser)?;
        if !opened {
            return Err(AuthError::Browser);
        }
        Ok(())
    }
}

/// A failed login, a refused grant or a redirect that never came.
///
/// No variant carries a token, a code or a body from the token endpoint: a notice
/// is a line in a status bar and possibly a log, and the only thing any of these
/// ever needs to say is what to do about it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// The loopback socket could not be bound. Rare, and not retryable by doing
    /// the same thing again.
    #[error("trak: could not listen on 127.0.0.1 for the Spotify login")]
    Bind,
    /// No browser could be opened, so no redirect can arrive.
    #[error("trak: could not open a browser for the Spotify login")]
    Browser,
    /// Nobody finished the login in the browser. The listener is closed and the
    /// port is free, so trying again is a fresh login rather than a resume.
    #[error("trak: the Spotify login timed out after {seconds}s")]
    Timeout {
        /// The whole budget this attempt was given, in whole seconds.
        seconds: u64,
    },
    /// Something other than a browser redirect arrived on the port.
    #[error("trak: something connected to the Spotify login that was not a redirect")]
    NoRedirect,
    /// The redirect carried a `state` that is not this attempt's, so the code in
    /// it belongs to somebody else's authorization. Nothing is exchanged.
    #[error("trak: the Spotify login came back with an unexpected state")]
    StateMismatch,
    /// The user did not consent, or Spotify redirected with an error instead of
    /// a code.
    #[error("trak: the Spotify login was not completed ({reason})")]
    Denied {
        /// Spotify's own `error` value, filtered down to something that cannot be
        /// anything else by the `error_code` filter below.
        reason: String,
    },
    /// The redirect was ours and it carried no code.
    #[error("trak: the Spotify login came back with no authorization code")]
    NoCode,
    /// The token endpoint would not accept the grant.
    #[error("trak: Spotify refused the login")]
    Refused,
    /// The token endpoint could not be reached, or answered with a fault. The
    /// stored token is untouched, because this is not its fault.
    #[error("trak: Spotify's token endpoint could not be reached")]
    Unreachable,
    /// The token endpoint answered, but not with a token.
    #[error("trak: Spotify's token endpoint did not return a token")]
    Malformed,
    /// The refresh token is spent or revoked (docs/WEB-API.md §6). This is a state
    /// the TUI turns into a login, not a fault it sits on.
    #[error("trak: Spotify rejected the stored refresh token")]
    RefreshRejected,
    /// The token could not be written to the file.
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl AuthError {
    /// One line, for a toast or a status bar. Never a stack trace, never a
    /// panic, and never any part of a credential.
    pub fn notice(&self) -> String {
        self.to_string()
    }

    /// Whether the only way forward is a new login.
    ///
    /// False for exactly three things, and they are the three that are not about
    /// the login: a socket trak could not bind, a network that did not answer,
    /// and a file that could not be written -- all of which a retry may fix on its
    /// own. Everything else, from a timeout to a refused grant to a spent refresh
    /// token, is something the user answers by logging in again, and saying so
    /// early is what keeps a dead connection from looking like a bug.
    pub fn needs_relogin(&self) -> bool {
        !matches!(
            self,
            AuthError::Bind | AuthError::Unreachable | AuthError::Malformed | AuthError::Store(_)
        )
    }
}

/// What every `Debug` print of a credential puts in its place.
const REDACTED: &str = "[redacted]";

/// A login that opens a browser.
///
/// Holds borrowed seams rather than owning them: the same endpoint and the same
/// store are used by [`Session`], and a login is not a second session.
pub struct Login<'a> {
    endpoint: &'a dyn TokenEndpoint,
    browser: &'a dyn Browser,
    store: &'a dyn Store,
    timeout: Duration,
}

impl<'a> Login<'a> {
    /// A login over the real seams, or the fakes a test gives it.
    pub fn new(
        endpoint: &'a dyn TokenEndpoint,
        browser: &'a dyn Browser,
        store: &'a dyn Store,
    ) -> Self {
        Self {
            endpoint,
            browser,
            store,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// How long the redirect may take. Tests use a fraction of a second; the
    /// browser-facing default is [`DEFAULT_TIMEOUT`].
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The whole flow: bind, open, receive, verify, exchange, save.
    ///
    /// `now` is the moment the user authorized, and it is passed in rather than
    /// read so the six-month window is arithmetic a test can pin. It is stamped
    /// from here, not from when the response came back, because the six months
    /// start at the authorization.
    pub fn run(
        &self,
        client_id: &str,
        scopes: &[&str],
        now: SystemTime,
    ) -> Result<Token, AuthError> {
        let pending = Loopback::bind(self.timeout)?;
        let pkce = Pkce::generate();
        // Read before the redirect consumes the listener, so the exchange repeats
        // the exact string the authorization request carried.
        let redirect_uri = pending.redirect_uri();
        self.browser.open(&authorize_url(
            client_id,
            &redirect_uri,
            pkce.state(),
            pkce.challenge(),
            scopes,
        ))?;
        let code = pending.wait(pkce.state())?;
        let response = self.endpoint.exchange(&CodeExchange {
            code: &code,
            redirect_uri: &redirect_uri,
            code_verifier: pkce.verifier(),
            client_id,
        })?;
        // A login with no refresh token is not a login: it would be a token that
        // silently dies in an hour with nothing in the file to renew it with.
        let token = Token::from_response(&response, now).ok_or(AuthError::Malformed)?;
        self.store.save(&token)?;
        Ok(token)
    }
}

/// The half of login that never opens a browser.
///
/// Refreshes a stale access token and writes the result back, so the next start
/// does not refresh again, and turns a spent refresh token into
/// [`Auth::Reconnect`] after discarding it. The local six-month record is a
/// *warning* here, never a gate: the server is the authority on whether a refresh
/// token works, and a clock that is a few days fast must not cost the user a
/// login.
pub struct Session<'a> {
    endpoint: &'a dyn TokenEndpoint,
    store: &'a dyn Store,
    client_id: &'a str,
}

impl<'a> Session<'a> {
    /// A session over the same seams a [`Login`] uses, for the Client ID in
    /// `config.toml`.
    pub fn new(endpoint: &'a dyn TokenEndpoint, store: &'a dyn Store, client_id: &'a str) -> Self {
        Self {
            endpoint,
            store,
            client_id,
        }
    }

    /// A token good enough to use now, refreshed if the stored one is stale.
    pub fn access(&self, token: &Token, now: SystemTime) -> Auth {
        if !token.access_stale(now) {
            return Auth::ready(token.clone());
        }
        self.refresh(token, now)
    }

    /// Spend the refresh token whether or not the access token is stale.
    pub fn refresh(&self, token: &Token, now: SystemTime) -> Auth {
        let request = RefreshRequest {
            refresh_token: token.refresh_token(),
            client_id: self.client_id,
        };
        match self.endpoint.refresh(&request) {
            Ok(response) => match token.refreshed(&response, now) {
                Some(refreshed) => match self.store.save(&refreshed) {
                    Ok(()) => Auth::ready(refreshed),
                    // The new token is real and this session can use it; only the
                    // file did not take it. Saying so is enough, and the next
                    // start refreshes again rather than believing an expiry that
                    // is already past.
                    Err(e) => Auth {
                        token: Some(refreshed),
                        outcome: AuthOutcome::NotSaved(AuthError::Store(e)),
                    },
                },
                None => Auth::failed(AuthError::Malformed),
            },
            // The one case the docs are explicit about: "your app should discard
            // the refresh token and start the appropriate reauthorization". The
            // file goes, so a spent token is not retried every hour for six
            // months, and the user is asked to log in again. A failure to delete
            // it does not change the answer -- the token is spent either way, and
            // the next load finds a file whose own six months are up.
            Err(AuthError::RefreshRejected) => {
                let _ = self.store.clear();
                Auth::reconnect("Spotify will not renew the stored token")
            }
            Err(e) => Auth::failed(e),
        }
    }
}

/// What an attempt to get a usable token produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Auth {
    /// The token to send, if there is one. A usable token and a problem with the
    /// file are not exclusive, and pretending they were would throw away a
    /// working session over a full disk.
    pub token: Option<Token>,
    /// What the attempt concluded.
    pub outcome: AuthOutcome,
}

/// Why an [`Auth`] is what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthOutcome {
    /// A token that works, on disk.
    Ready,
    /// A token that works, but the file could not be updated. This session
    /// continues; the next start refreshes again.
    NotSaved(AuthError),
    /// The refresh token is spent or revoked and has been discarded. The user has
    /// to log in again, and this is a state to show, not a failure to sit on --
    /// a six-month-old token that cannot be renewed is the normal end of a
    /// connection (docs/WEB-API.md §6), not an incident.
    Reconnect(&'static str),
    /// Something failed and the stored token is exactly as it was. Worth
    /// retrying.
    Unavailable(AuthError),
}

impl Auth {
    fn ready(token: Token) -> Self {
        Self {
            token: Some(token),
            outcome: AuthOutcome::Ready,
        }
    }

    fn failed(error: AuthError) -> Self {
        Self {
            token: None,
            outcome: AuthOutcome::Unavailable(error),
        }
    }

    fn reconnect(reason: &'static str) -> Self {
        Self {
            token: None,
            outcome: AuthOutcome::Reconnect(reason),
        }
    }

    /// The token to send, if there is one.
    pub fn token(&self) -> Option<&Token> {
        self.token.as_ref()
    }

    /// Whether the next thing to do is a login.
    pub fn needs_relogin(&self) -> bool {
        matches!(self.outcome, AuthOutcome::Reconnect(_))
    }

    /// One line for a status bar, and nothing at all when there is nothing to
    /// say. The two states that need a login are phrased as one.
    pub fn notice(&self) -> Option<String> {
        match &self.outcome {
            AuthOutcome::Ready => None,
            AuthOutcome::NotSaved(e) => Some(format!("{}; this session is fine", e.notice())),
            AuthOutcome::Reconnect(reason) => Some(format!(
                "trak: reconnect Spotify ({reason}); the stored token has been discarded"
            )),
            AuthOutcome::Unavailable(e) => Some(e.notice()),
        }
    }
}

/// The loopback socket the browser is redirected back to.
///
/// Bound on an ephemeral port, because the registered redirect URI has no port
/// and a dynamic one is only allowed for a loopback IP literal
/// (docs/WEB-API.md §1). Everything about it is `127.0.0.1`: `localhost` is
/// explicitly not allowed as a redirect URI, and this is where that rule would
/// otherwise be easiest to break by writing `localhost` once.
struct Loopback {
    listener: TcpListener,
    port: u16,
    timeout: Duration,
}

impl Loopback {
    fn bind(timeout: Duration) -> Result<Self, AuthError> {
        let listener = TcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))
            .map_err(|_| AuthError::Bind)?;
        // Non-blocking so the wait is a poll against a deadline rather than an
        // `accept` that can sit forever. The alternative is a thread per login
        // and a way to abandon one.
        listener
            .set_nonblocking(true)
            .map_err(|_| AuthError::Bind)?;
        let port = listener.local_addr().map_err(|_| AuthError::Bind)?.port();
        Ok(Self {
            listener,
            port,
            timeout,
        })
    }

    /// The `redirect_uri` for this attempt: the registered URI with this
    /// attempt's port on it.
    fn redirect_uri(&self) -> String {
        format!("http://{}:{}", Ipv4Addr::LOCALHOST, self.port)
    }

    /// Wait for the redirect and return the code in it, having checked that the
    /// `state` is ours.
    ///
    /// Takes `self`, so the listener is closed on every path out of here --
    /// matched, mismatched or timed out -- and the port is back in the pool before
    /// a single request goes to Spotify. A login that leaves a socket open would
    /// make the next attempt's "bind an ephemeral port" a coin toss on a busy
    /// machine.
    fn wait(self, expected_state: &str) -> Result<String, AuthError> {
        let deadline = deadline(self.timeout);
        let (mut stream, target) = self.accept_one(deadline)?;
        match authorize_code(&target, expected_state) {
            Ok(code) => {
                reply(&mut stream, true);
                Ok(code)
            }
            Err(e) => {
                // Say what happened, so the browser tab is not left showing a
                // spinner or a half-loaded page that looks like it worked.
                reply(&mut stream, false);
                Err(e)
            }
        }
    }

    fn accept_one(self, deadline: Option<Instant>) -> Result<(TcpStream, String), AuthError> {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let mut stream = stream;
                    let target = read_target(&mut stream, deadline)?;
                    return Ok((stream, target));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if is_past(deadline) {
                        return Err(AuthError::Timeout {
                            seconds: self.timeout.as_secs(),
                        });
                    }
                    sleep(POLL);
                }
                // A connection that went away between arriving and being handed
                // over. A browser that finished and closed its socket does this,
                // and so does anything else that finds a port, so it is not a
                // failure of the listener: the wait carries on. The numbers are
                // the same two errors on macOS and on Linux, in a different order.
                Err(e) if connection_vanished(&e) => {
                    if is_past(deadline) {
                        return Err(AuthError::Timeout {
                            seconds: self.timeout.as_secs(),
                        });
                    }
                    sleep(POLL);
                }
                Err(_) => return Err(AuthError::NoRedirect),
            }
        }
    }
}

/// `ECONNABORTED` and `ECONNRESET`: 53 and 54 on macOS, 103 and 104 on Linux.
/// Spelled out rather than pulled from a libc binding trak does not have, because
/// the alternative is treating a browser that closed its socket as a listener that
/// is broken.
fn connection_vanished(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(53) | Some(54) | Some(103) | Some(104)
    )
}

/// When the wait is over. `Instant + Duration` panics on overflow, and this is
/// reached from a caller-supplied timeout, so a budget past the representable
/// range is treated as no budget rather than as a panic. `None` means "already
/// over".
fn deadline(timeout: Duration) -> Option<Instant> {
    Instant::now().checked_add(timeout)
}

fn is_past(deadline: Option<Instant>) -> bool {
    match deadline {
        Some(when) => Instant::now() >= when,
        None => true,
    }
}

/// Read one request off the loopback socket and hand back its target, without
/// answering: whether there is anything to say depends on what it says.
fn read_target(stream: &mut TcpStream, deadline: Option<Instant>) -> Result<String, AuthError> {
    // A socket accepted from a *non-blocking* listener is itself non-blocking on
    // macOS and blocking on Linux, and the only portable way to be sure is to say
    // so. Left alone, the first read of a fast callback finds no data yet, gets
    // `EWOULDBLOCK`, and a login that is working reports "not a redirect".
    stream
        .set_nonblocking(false)
        .map_err(|_| AuthError::NoRedirect)?;
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|_| AuthError::NoRedirect)?;
    // A socket that connects and then says nothing is not a browser. This is how
    // long that is tolerated, and it is a *separate* budget from the login's: a
    // client that has gone quiet is answered now, while a client that is still
    // typing gets the whole of the login's time.
    let mut quiet_since = Instant::now().checked_add(READ_TIMEOUT);
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                head.extend_from_slice(&chunk[..n]);
                quiet_since = Instant::now().checked_add(READ_TIMEOUT);
                if head.windows(4).any(|w| w == b"\r\n\r\n") || head.len() >= MAX_HEAD {
                    break;
                }
            }
            // Nothing yet, or a read that timed out. Either way the wait is on the
            // budgets rather than on the read, because a half-sent request is not
            // a redirect and a request that has not started yet is not a failure.
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                if is_past(quiet_since) || is_past(deadline) {
                    break;
                }
                sleep(POLL);
            }
            Err(_) => return Err(AuthError::NoRedirect),
        }
    }
    if head.is_empty() {
        return Err(AuthError::NoRedirect);
    }
    let head = String::from_utf8_lossy(&head);
    // `GET /?code=...&state=... HTTP/1.1`: the target is the only part that says
    // anything about the authorization.
    head.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .map(str::to_string)
        .ok_or(AuthError::NoRedirect)
}

/// The code out of a redirect target, or why there is not one.
///
/// The order is the security-relevant part. `state` is checked before the code is
/// looked at and before anything is sent to Spotify, so a code delivered to this
/// port by anything that is not the request trak made is never exchanged -- and
/// exchanging it would be exchanging somebody else's authorization for trak's
/// account.
fn authorize_code(target: &str, expected_state: &str) -> Result<String, AuthError> {
    let params = query_params(target);
    if let Some(reason) = params
        .iter()
        .find_map(|(k, v)| (k == "error").then(|| error_code(v)))
    {
        return Err(AuthError::Denied { reason });
    }
    let state = params
        .iter()
        .find_map(|(k, v)| (k == "state").then(|| v.clone()));
    if state.as_deref() != Some(expected_state) {
        return Err(AuthError::StateMismatch);
    }
    params
        .iter()
        .find_map(|(k, v)| (k == "code").then(|| v.clone()))
        .filter(|code| !code.is_empty())
        .ok_or(AuthError::NoCode)
}

/// The `error` value, reduced to something that cannot be a credential.
///
/// The redirect lands on a loopback port that anything local can reach, so the
/// value is treated as hostile input: it is bounded, and anything that is not a
/// letter or an underscore is dropped, which is all Spotify's own values
/// (`access_denied`) are made of. Without this, a page that answered
/// `?error=<a token>` would put a token in a status line.
fn error_code(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if out.len() >= 32 {
            break;
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            out.push(ch);
        }
    }
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

/// The query of a request target, percent-decoded.
fn query_params(target: &str) -> Vec<(String, String)> {
    let Some((_, query)) = target.split_once('?') else {
        return Vec::new();
    };
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            None => (percent_decode(pair), String::new()),
        })
        .collect()
}

/// The page the browser ends up on. It has to be a real HTTP response with a
/// `Content-Length`, or the tab sits there looking broken; and it must never
/// contain anything from the request, because a page rendered from a token is a
/// token in a screenshot.
const CLOSED: &str = "<!doctype html><meta charset=utf-8><title>trak</title>\
<p>Spotify is connected. You can close this tab.</p>";

/// The page for a login that did not complete, for the same reasons.
const NOT_DONE: &str = "<!doctype html><meta charset=utf-8><title>trak</title>\
<p>That Spotify login did not complete. Nothing has changed; you can close this tab and try again from trak.</p>";

/// Answer the browser.
///
/// Always a 200 with a small page of its own, even when the login did not
/// complete: the tab has to land on something a person can read, and an error
/// status would be replaced by the browser's own page, which says less than this
/// does. The page never contains anything from the request.
fn reply(stream: &mut TcpStream, ok: bool) {
    let body = if ok { CLOSED } else { NOT_DONE };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    // Best effort by design: the browser may have gone already, and a login that
    // worked must not fail because the tab it came from was closed.
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

/// Percent-encode a query or form value.
///
/// Everything outside the unreserved set is escaped, so a `:` or `/` in a redirect
/// URI, a space in a scope list, and a `&` in anything a future field carries all
/// leave the value's shape alone. Copied in spirit from `lyrics.rs`'s encoder,
/// which is private to it.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(*byte));
            }
            other => {
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("%{other:02X}"));
            }
        }
    }
    out
}

/// The inverse of [`percent_encode`], plus `+` as a space because that is what a
/// form-encoded value uses.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match hex_pair(&bytes[i + 1], &bytes[i + 2]) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    // A `%` that is not two hex digits is not ours; keep it as it
                    // came rather than dropping a byte of someone's value.
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_pair(high: &u8, low: &u8) -> Option<u8> {
    fn digit(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    Some(digit(*high)? * 16 + digit(*low)?)
}

fn random(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::net::TcpListener as StdListener;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};

    use crate::config::Paths;
    use crate::web::token::TokenFile;

    /// RFC 7636's own worked example, verbatim. Pinned rather than computed in
    /// the test, because a test that recomputes the expected value with the same
    /// code proves only that the code is self-consistent.
    const RFC_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const RFC_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    const CLIENT: &str = "4c2b19f7a1e94d3b8a0f2c6d5e7b9f31";
    const CODE: &str = "AQD8tN-4x9_example-code-value";
    const ACCESS: &str = "BQAAtoken-access-value";
    const REFRESH: &str = "AQD-refresh-value";

    /// A token endpoint that answers from memory, over a real loopback socket, so
    /// the form encoding, the HTTP round trip and the response parsing are all
    /// covered for real without a network.
    struct MockEndpoint {
        url: String,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl MockEndpoint {
        /// A server that answers every request with `status` and `body`, and
        /// records what it was sent.
        fn new(status: u16, body: String) -> Self {
            let listener = StdListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("mock binds");
            listener.set_nonblocking(true).expect("mock nonblocking");
            let port = listener.local_addr().expect("mock addr").port();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let seen = Arc::clone(&requests);
            let flag = Arc::clone(&stop);
            let thread = thread::spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            // The same trap the production listener has to avoid:
                            // a socket accepted from a non-blocking listener is
                            // non-blocking on macOS, so a request that has not
                            // arrived yet reads as "no request" rather than as
                            // "not yet".
                            let _ = stream.set_nonblocking(false);
                            if let Some(request) = read_request(&mut stream) {
                                seen.lock().expect("mock lock").push(request);
                                let _ = stream.write_all(http_response(status, &body).as_bytes());
                                let _ = stream.flush();
                            }
                        }
                        // Any accept error is this mock's problem, not the test's:
                        // a listener that cannot take a connection for a moment
                        // is not a mock that has stopped answering, and dying here
                        // would turn into a slow "could not be reached" in
                        // whichever test happened to run next.
                        Err(_) => thread::sleep(Duration::from_millis(2)),
                    }
                }
            });
            Self {
                url: format!("http://{}:{port}/api/token", Ipv4Addr::LOCALHOST),
                requests,
                stop,
                thread: Some(thread),
            }
        }

        /// The endpoint that hands out a token, the shape Spotify's token
        /// endpoint returns. One line, because a raw string's trailing `\` is a
        /// backslash and not a line continuation -- and a stray backslash makes
        /// the body JSON no client can parse.
        fn granting() -> Self {
            Self::new(
                200,
                format!(
                    r#"{{"access_token":"{ACCESS}","token_type":"Bearer","expires_in":3600,"refresh_token":"{REFRESH}","scope":"user-library-read playlist-read-private"}}"#
                ),
            )
        }

        /// An endpoint that has never heard of this refresh token.
        fn refusing() -> Self {
            Self::new(
                400,
                r#"{"error":"invalid_grant","error_description":"Refresh token revoked"}"#
                    .to_string(),
            )
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().expect("mock lock").clone()
        }
    }

    impl Drop for MockEndpoint {
        fn drop(&mut self) {
            // Unblock the accept loop, so no test leaves a thread parked on a
            // socket for the rest of the run.
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// One request, as text: enough for a test to assert on the form body. Reads
    /// the headers and then exactly the number of bytes they promise, because a
    /// body cut short is a JSON document that does not parse and a test failure
    /// that looks like a product bug.
    fn read_request(stream: &mut TcpStream) -> Option<String> {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let mut request = Vec::new();
        let mut chunk = [0u8; 1024];
        let mut end = None;
        while end.is_none() {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    request.extend_from_slice(&chunk[..n]);
                    end = request
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .map(|at| at + 4);
                }
                Err(_) => break,
            }
        }
        let end = end?;
        let length = content_length(&request[..end]);
        while request.len() < end + length {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => request.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
        Some(String::from_utf8_lossy(&request).into_owned())
    }

    fn content_length(headers: &[u8]) -> usize {
        String::from_utf8_lossy(headers)
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse().ok())
            .unwrap_or(0)
    }

    fn http_response(status: u16, body: &str) -> String {
        let reason = match status {
            200 => "OK",
            400 => "Bad Request",
            _ => "Error",
        };
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// What a [`FakeBrowser`] does when trak opens a URL: send the redirect back,
    /// or fail the way a machine with no browser would.
    type OnOpen = Box<dyn Fn(&str) -> Result<(), AuthError> + Send + Sync>;

    /// A browser that answers the redirect itself: it reads the port and the
    /// state out of the URL trak built and sends a real HTTP request to it, so the
    /// loopback path under test is the one that ships.
    struct FakeBrowser {
        on_open: OnOpen,
        opened: Arc<Mutex<Vec<String>>>,
    }

    impl FakeBrowser {
        /// Completes the login with the state trak put in the URL.
        fn consenting(code: &str) -> Self {
            let code = code.to_string();
            Self::scripted(move |url| respond(url, &code, None))
        }

        /// Completes it with a `state` of its own choosing.
        fn with_state(state: &str, code: &str) -> Self {
            let (state, code) = (state.to_string(), code.to_string());
            Self::scripted(move |url| respond(url, &code, Some(&state)))
        }

        /// Answers with an `error` instead of a code, which is what a refused
        /// consent looks like.
        fn denying(value: &str) -> Self {
            let value = value.to_string();
            Self::scripted(move |url| redirect(url, &format!("error={value}&state={{state}}")))
        }

        /// Sends a callback that is missing the code.
        fn without_code() -> Self {
            Self::scripted(|url| redirect(url, "state={state}"))
        }

        /// Opens the URL and does nothing else, which is a user who has not
        /// finished.
        fn silent() -> Self {
            Self::scripted(|_| Ok(()))
        }

        /// Fails to open anything.
        fn broken() -> Self {
            Self::scripted(|_| Err(AuthError::Browser))
        }

        fn scripted(
            on_open: impl Fn(&str) -> Result<(), AuthError> + Send + Sync + 'static,
        ) -> Self {
            Self {
                on_open: Box::new(on_open),
                opened: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// Every URL trak tried to open, in order.
        fn opened(&self) -> Vec<String> {
            self.opened.lock().expect("opened").clone()
        }
    }

    impl Browser for FakeBrowser {
        fn open(&self, url: &str) -> Result<(), AuthError> {
            self.opened.lock().expect("opened").push(url.to_string());
            (self.on_open)(url)
        }
    }

    /// A callback that echoes the request's own `state`, so the happy path does
    /// not have to know anything about the value trak generated.
    fn respond(url: &str, code: &str, state: Option<&str>) -> Result<(), AuthError> {
        let state = match state {
            Some(state) => state.to_string(),
            None => param(url, "state").ok_or(AuthError::NoCode)?,
        };
        let query = format!("code={code}&state={state}");
        send_to(url, &query)
    }

    fn redirect(url: &str, query: &str) -> Result<(), AuthError> {
        let query = query.replace("{state}", &param(url, "state").unwrap_or_default());
        send_to(url, &query)
    }

    fn send_to(url: &str, query: &str) -> Result<(), AuthError> {
        let port = param(url, "redirect_uri")
            .and_then(|uri| uri.rsplit(':').next().and_then(|p| p.parse().ok()))
            .ok_or(AuthError::Bind)?;
        let mut stream =
            TcpStream::connect((Ipv4Addr::LOCALHOST, port)).map_err(|_| AuthError::Bind)?;
        let request =
            format!("GET /?{query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: test\r\n\r\n");
        stream
            .write_all(request.as_bytes())
            .map_err(|_| AuthError::NoRedirect)?;
        // The reply is trak's to write and it closes the socket, so nothing waits
        // for one: reading here would block a fake browser on a real one.
        Ok(())
    }

    /// One query value out of a URL, decoded. A test-only parser, small enough to
    /// be obviously right.
    fn param(url: &str, key: &str) -> Option<String> {
        let (_, query) = url.split_once('?')?;
        field(query, key)
    }

    /// One field out of an `application/x-www-form-urlencoded` body, which is a
    /// query string with no `?` in front of it.
    fn form_field(body: &str, key: &str) -> Option<String> {
        field(body, key)
    }

    fn field(query: &str, key: &str) -> Option<String> {
        query.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == key).then(|| percent_decode(v))
        })
    }

    /// A config directory that removes itself, and the store into it. Nothing here
    /// can reach the owner's own `~/.config/trak`.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "trak-auth-test-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sandbox(tag: &str) -> (TempDir, TokenFile) {
        let home = TempDir::new(tag);
        let store = TokenFile::at(&Paths::from_vars(None, Some(home.0.clone())));
        (home, store)
    }

    fn at(secs: u64) -> SystemTime {
        std::time::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + secs)
    }

    /// The scopes a caller would ask for. The list itself is TODO 7.3's, from the
    /// per-endpoint table in `docs/WEB-API.md` §3; what matters here is that a
    /// space-separated list survives the trip.
    const SCOPES: &[&str] = &["user-library-read", "playlist-read-private"];

    // ------------------------------------------------------------------ pkce

    #[test]
    fn a_verifier_is_pkce_shaped() {
        for _ in 0..16 {
            let pkce = Pkce::generate();
            // 43 to 128 characters (RFC 7636 §4.1), from `[A-Za-z0-9-._~]`.
            assert!(
                (43..=128).contains(&pkce.verifier().len()),
                "{} chars",
                pkce.verifier().len()
            );
            assert!(
                pkce.verifier()
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c)),
                "{:?} is outside the allowed set",
                pkce.verifier()
            );
            assert!(!pkce.verifier().contains('='), "unpadded");
        }
    }

    #[test]
    fn the_challenge_is_the_s256_of_the_verifier() {
        assert_eq!(challenge_for(RFC_VERIFIER), RFC_CHALLENGE);
        // And the same two steps as one call, so `generate` cannot hash something
        // other than the verifier it hands out.
        let pkce = Pkce::generate();
        assert_eq!(pkce.challenge(), challenge_for(pkce.verifier()));
        assert_eq!(pkce.challenge().len(), 43, "unpadded base64url");
    }

    #[test]
    fn base64url_is_unpadded_and_url_safe() {
        // Bytes chosen to contain the standard alphabet's `+` and `/`.
        let encoded = base64url(&[0xfb, 0xff, 0xbe, 0x00, 0x3e, 0x3f]);
        assert_eq!(encoded, "-_--AD4_");
        assert!(!encoded.contains('='), "padding breaks a query value");
        assert!(!encoded.contains('+') && !encoded.contains('/'), "url-safe");
        assert_eq!(base64url(&[]), "");
    }

    #[test]
    fn two_logins_share_nothing() {
        let one = Pkce::generate();
        let two = Pkce::generate();
        for (name, a, b) in [
            ("verifier", one.verifier(), two.verifier()),
            ("challenge", one.challenge(), two.challenge()),
            ("state", one.state(), two.state()),
        ] {
            assert_ne!(a, b, "{name} is reused");
        }
    }

    #[test]
    fn pkce_debug_redacts_the_verifier() {
        let pkce = Pkce::generate();
        let debug = format!("{pkce:?}");
        assert!(
            !debug.contains(pkce.verifier()),
            "{debug:?} leaked a verifier"
        );
        assert!(debug.contains("redacted"), "{debug:?}");
    }

    // ------------------------------------------------------------------ urls

    /// The rule `docs/WEB-API.md` §1 puts in bold, asserted on every URL this
    /// module builds. `localhost` is not a redirect URI Spotify accepts, and a
    /// login that used it would fail at the one step a user has to be present for.
    #[test]
    fn no_request_trak_makes_says_localhost() {
        let url = authorize_url(
            CLIENT,
            "http://127.0.0.1:53219",
            "state-value",
            "challenge-value",
            SCOPES,
        );
        assert!(!url.contains("localhost"), "{url}");
        assert!(url.starts_with(AUTHORIZE_ENDPOINT), "{url}");
        assert_eq!(
            param(&url, "redirect_uri").as_deref(),
            Some("http://127.0.0.1:53219")
        );
        assert_eq!(
            param(&url, "code_challenge_method").as_deref(),
            Some("S256")
        );
        assert_eq!(param(&url, "response_type").as_deref(), Some("code"));
        assert_eq!(param(&url, "client_id").as_deref(), Some(CLIENT));
        assert_eq!(
            param(&url, "scope").as_deref(),
            Some("user-library-read playlist-read-private")
        );
    }

    /// The registered URI, pinned to the exact form §1 says to register: an
    /// explicit loopback IP literal, with no port and no path after the scheme.
    /// A port here would need a fixed port at login time, and a path is the one
    /// thing the doc flags as unconfirmed.
    #[test]
    fn the_registered_redirect_uri_is_a_loopback_literal_with_no_port_and_no_path() {
        assert_eq!(REGISTERED_REDIRECT_URI, "http://127.0.0.1");
        let authority = REGISTERED_REDIRECT_URI
            .strip_prefix("http://")
            .expect("http");
        assert_eq!(authority, "127.0.0.1", "an explicit IP literal");
        assert!(!authority.contains(':'), "no port");
        assert!(!authority.contains('/'), "no path");
        assert!(!REGISTERED_REDIRECT_URI.contains("localhost"));
    }

    /// The port is bound per login and sent in the request, which is the only
    /// reason the no-port registration is legal.
    #[test]
    fn a_login_binds_an_ephemeral_loopback_port() {
        let one = Loopback::bind(Duration::from_millis(1)).expect("bind");
        let two = Loopback::bind(Duration::from_millis(1)).expect("bind");
        assert_ne!(one.port, 0, "an ephemeral port, not 0");
        assert!(one.redirect_uri().starts_with("http://127.0.0.1:"));
        assert_ne!(
            one.redirect_uri(),
            two.redirect_uri(),
            "two logins do not fight over a port"
        );
    }

    // ----------------------------------------------------------------- login

    /// The whole flow, for real: a real loopback listener, a real HTTP callback
    /// from the browser, and a real form POST to a token endpoint that is a
    /// loopback socket. Nothing touches the network and nothing opens.
    #[test]
    fn a_full_login_against_a_mock_token_endpoint() {
        let (_home, store) = sandbox("login");
        let endpoint = MockEndpoint::granting();
        let client = SpotifyEndpoint::at(&endpoint.url);
        let browser = FakeBrowser::consenting(CODE);
        let login = Login::new(&client, &browser, &store).with_timeout(Duration::from_secs(5));

        let token = login
            .run(CLIENT, SCOPES, at(0))
            .expect("the login completes");

        assert_eq!(token.access_token(), ACCESS);
        assert_eq!(token.refresh_token(), REFRESH);
        assert_eq!(token.scope(), "user-library-read playlist-read-private");
        assert_eq!(token.authorized_at(), at(0), "the six months start here");
        // And it is on disk, not just in memory.
        let stored = store.load().expect("load");
        assert_eq!(stored.token.as_ref(), Some(&token));
        assert!(stored.notice(at(0)).is_none());

        // The browser went to the authorize endpoint with this login's challenge.
        let opened = browser.opened();
        assert_eq!(opened.len(), 1, "{opened:?}");
        assert!(opened[0].starts_with(AUTHORIZE_ENDPOINT), "{opened:?}");
        let challenge = param(&opened[0], "code_challenge").expect("challenge");
        assert!(!challenge.is_empty());
        assert!(!opened[0].contains("localhost"), "{opened:?}");

        // The token request carried the flow's own fields and this attempt's
        // redirect, and nothing named localhost.
        let requests = endpoint.requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        let body = requests[0].split("\r\n\r\n").nth(1).unwrap_or_default();
        assert!(!body.contains("localhost"), "{body}");
        for field in [
            "grant_type=authorization_code",
            "client_id=",
            "code_verifier=",
        ] {
            assert!(body.contains(field), "{field} is missing from {body}");
        }
        let redirect = form_field(body, "redirect_uri").expect("redirect_uri");
        assert!(redirect.starts_with("http://127.0.0.1:"), "{redirect}");
        assert_eq!(form_field(body, "code").as_deref(), Some(CODE));
        assert_eq!(form_field(body, "client_id").as_deref(), Some(CLIENT));
        // The verifier that was hashed into the challenge is the one sent.
        let verifier = form_field(body, "code_verifier").expect("verifier");
        assert_eq!(challenge_for(&verifier), challenge);
    }

    /// The one security-relevant thing in the flow: a code delivered to trak's
    /// port by anything other than trak's own request is not exchanged, and
    /// nothing is sent to Spotify at all.
    #[test]
    fn a_state_that_is_not_ours_aborts_without_exchanging() {
        let (_home, store) = sandbox("state");
        let endpoint = MockEndpoint::granting();
        let client = SpotifyEndpoint::at(&endpoint.url);
        let browser = FakeBrowser::with_state("not-the-state-trak-sent", CODE);
        let login = Login::new(&client, &browser, &store).with_timeout(Duration::from_secs(5));

        let error = login.run(CLIENT, SCOPES, at(0)).expect_err("no token");

        assert_eq!(error, AuthError::StateMismatch);
        assert!(error.needs_relogin());
        assert!(endpoint.requests().is_empty(), "{:?}", endpoint.requests());
        assert!(
            store.load().expect("load").token.is_none(),
            "and nothing was written"
        );
    }

    /// A user who closed the tab. The listener must not hold the login, and the
    /// port must be back in the pool -- a login that leaks its socket makes the
    /// next attempt's ephemeral bind a coin toss.
    #[test]
    fn the_redirect_times_out_and_frees_the_port() {
        let (_home, store) = sandbox("timeout");
        let endpoint = MockEndpoint::granting();
        let client = SpotifyEndpoint::at(&endpoint.url);
        let browser = FakeBrowser::silent();
        let budget = Duration::from_millis(150);
        let login = Login::new(&client, &browser, &store).with_timeout(budget);

        let started = Instant::now();
        let error = login.run(CLIENT, SCOPES, at(0)).expect_err("no token");
        let elapsed = started.elapsed();

        assert_eq!(error, AuthError::Timeout { seconds: 0 });
        assert!(
            elapsed < Duration::from_secs(5),
            "it waited {elapsed:?}, which is hanging"
        );
        assert!(endpoint.requests().is_empty());

        // The port the login bound is free again.
        let port = param(browser.opened().first().expect("url"), "redirect_uri")
            .and_then(|uri| uri.rsplit(':').next().and_then(|p| p.parse().ok()))
            .expect("a port to check");
        StdListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)))
            .expect("the port was released");
    }

    /// Consent refused, and a redirect that is not one. Both are answers, not
    /// crashes, and neither reaches the token endpoint.
    #[test]
    fn a_login_that_was_not_completed_says_so() {
        let (_home, store) = sandbox("denied");
        let endpoint = MockEndpoint::granting();
        let client = SpotifyEndpoint::at(&endpoint.url);

        let browser = FakeBrowser::denying("access_denied");
        let error = Login::new(&client, &browser, &store)
            .with_timeout(Duration::from_secs(5))
            .run(CLIENT, SCOPES, at(0))
            .expect_err("no token");
        assert_eq!(
            error,
            AuthError::Denied {
                reason: "access_denied".to_string()
            }
        );
        assert!(
            error.notice().contains("access_denied"),
            "{}",
            error.notice()
        );

        let browser = FakeBrowser::without_code();
        let error = Login::new(&client, &browser, &store)
            .with_timeout(Duration::from_secs(5))
            .run(CLIENT, SCOPES, at(0))
            .expect_err("no token");
        assert_eq!(error, AuthError::NoCode);
        assert!(endpoint.requests().is_empty());
    }

    /// The redirect is a loopback port any local process can reach, so the `error`
    /// value is hostile input: it ends up in a status line, and a page that
    /// answered `?error=<a token>` must not be able to put a token there.
    #[test]
    fn a_denied_error_value_cannot_carry_a_token_into_a_notice() {
        let (_home, store) = sandbox("hostile-error");
        let endpoint = MockEndpoint::granting();
        let client = SpotifyEndpoint::at(&endpoint.url);
        let browser = FakeBrowser::denying(&format!("{ACCESS}-and-more"));
        let error = Login::new(&client, &browser, &store)
            .with_timeout(Duration::from_secs(5))
            .run(CLIENT, SCOPES, at(0))
            .expect_err("no token");
        assert!(!error.notice().contains(ACCESS), "{}", error.notice());
        assert!(
            error.notice().contains("not completed"),
            "{}",
            error.notice()
        );
    }

    #[test]
    fn a_browser_that_will_not_open_is_its_own_error() {
        let (_home, store) = sandbox("no-browser");
        let endpoint = MockEndpoint::granting();
        let client = SpotifyEndpoint::at(&endpoint.url);
        let error = Login::new(&client, &FakeBrowser::broken(), &store)
            .run(CLIENT, SCOPES, at(0))
            .expect_err("no token");
        assert_eq!(error, AuthError::Browser);
        assert!(endpoint.requests().is_empty());
    }

    /// Something that is not a browser: a connection that says nothing a redirect
    /// could say. The socket must not be able to hang the login, and it must not
    /// be able to reach the token endpoint.
    #[test]
    fn a_connection_that_is_not_a_redirect_is_refused() {
        let pending = Loopback::bind(Duration::from_secs(5)).expect("bind");
        let port = pending.port;
        // The port is connected to from another thread because `wait` is the call
        // under test and it is the one that blocks.
        let rude = thread::spawn(move || {
            let mut stream =
                TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect to the login");
            let _ = stream.write_all(b"HELLO\r\n\r\n");
            // Held until the login has answered or given up, so the socket is not
            // closed out from under the read.
            thread::sleep(Duration::from_secs(1));
        });
        let error = pending.wait("state").expect_err("no code");
        let _ = rude.join();
        assert_eq!(error, AuthError::NoRedirect);
        assert!(error.needs_relogin(), "a new attempt is the only way on");
    }

    // -------------------------------------------------------------- refresh

    /// A stale access token is refreshed and written back, so the next start does
    /// not refresh again -- and the six-month clock does not move.
    #[test]
    fn a_stale_access_token_is_refreshed_and_saved() {
        let (_home, store) = sandbox("refresh");
        let endpoint = MockEndpoint::granting();
        let client = SpotifyEndpoint::at(&endpoint.url);
        let authorized = at(0);
        store
            .save(&Token::from_response(&granted(), authorized).expect("token"))
            .expect("save");
        let stored = store.load().expect("load").token.expect("a token");

        // Two hours later: the access token is long gone, the refresh token is not.
        let now = at(7200);
        assert!(stored.access_stale(now));
        let auth = Session::new(&client, &store, CLIENT).access(&stored, now);

        assert_eq!(auth.outcome, AuthOutcome::Ready);
        let refreshed = auth.token().expect("a token");
        assert_eq!(
            refreshed.authorized_at(),
            authorized,
            "six months, unchanged"
        );
        assert_eq!(refreshed.refresh_token(), REFRESH);
        assert_eq!(
            store.load().expect("load").token,
            Some(refreshed.clone()),
            "and it is on disk"
        );
        assert!(!refreshed.access_stale(now), "the new hour is usable");
        let body = endpoint.requests()[0]
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or_default()
            .to_string();
        assert!(body.contains("grant_type=refresh_token"), "{body}");
        assert!(!body.contains("localhost"), "{body}");
    }

    #[test]
    fn an_access_token_that_is_not_stale_is_not_refreshed() {
        let (_home, store) = sandbox("fresh");
        let endpoint = MockEndpoint::granting();
        let client = SpotifyEndpoint::at(&endpoint.url);
        let token = Token::from_response(&granted(), at(0)).expect("token");
        let auth = Session::new(&client, &store, CLIENT).access(&token, at(60));
        assert_eq!(auth.outcome, AuthOutcome::Ready);
        assert_eq!(auth.token(), Some(&token));
        assert!(auth.notice().is_none());
        assert!(
            endpoint.requests().is_empty(),
            "a token an hour old is used"
        );
    }

    /// The judgment the whole reconnect design rests on: the local six-month
    /// record warns, and the server decides. A refresh token trak believes is
    /// spent is still offered to Spotify, because a clock that is a few days fast
    /// must not cost the user a login. Only Spotify's answer can end a
    /// connection.
    #[test]
    fn a_six_month_old_token_is_still_offered_to_spotify() {
        let (_home, store) = sandbox("six-months");
        let endpoint = MockEndpoint::granting();
        let client = SpotifyEndpoint::at(&endpoint.url);
        let authorized = at(0);
        let token = Token::from_response(&granted(), authorized).expect("token");
        let spent = at(crate::web::token::REFRESH_TOKEN_LIFETIME.as_secs() + 86_400);
        assert!(token.refresh_stale(spent), "the local record says so");

        let auth = Session::new(&client, &store, CLIENT).access(&token, spent);

        assert_eq!(
            auth.outcome,
            AuthOutcome::Ready,
            "the local clock warns; it does not gate"
        );
        assert_eq!(endpoint.requests().len(), 1, "it was offered");
    }

    /// The case `docs/WEB-API.md` §6 is explicit about: an invalid or expired
    /// refresh token is discarded and the user reauthorizes. Not an error state to
    /// sit in, and nothing is left behind to retry with.
    #[test]
    fn a_rejected_refresh_token_is_discarded_and_asks_for_a_login() {
        let (_home, store) = sandbox("rejected");
        let endpoint = MockEndpoint::refusing();
        let client = SpotifyEndpoint::at(&endpoint.url);
        let token = Token::from_response(&granted(), at(0)).expect("token");
        store.save(&token).expect("save");

        let auth = Session::new(&client, &store, CLIENT).access(&token, at(7200));

        assert_eq!(auth.token(), None);
        assert!(auth.needs_relogin());
        assert!(!matches!(auth.outcome, AuthOutcome::Unavailable(_)));
        assert!(!store.path().exists(), "the spent token is gone");
        let notice = auth.notice().expect("something to say");
        assert!(notice.contains("reconnect Spotify"), "{notice}");
        assert!(notice.contains("discarded"), "{notice}");
    }

    /// The other end of the same endpoint, and the reason the two cases are
    /// separate methods: a 5xx is the network's problem, so the token is left
    /// exactly where it is and the attempt is worth retrying.
    #[test]
    fn a_transient_failure_leaves_the_token_alone() {
        let (_home, store) = sandbox("transient");
        let endpoint = MockEndpoint::new(503, "{\"error\":{\"status\":503}}".to_string());
        let client = SpotifyEndpoint::at(&endpoint.url);
        let token = Token::from_response(&granted(), at(0)).expect("token");
        store.save(&token).expect("save");

        let auth = Session::new(&client, &store, CLIENT).access(&token, at(7200));

        assert_eq!(auth.token(), None);
        assert!(!auth.needs_relogin());
        assert_eq!(
            auth.notice().as_deref(),
            Some(AuthError::Unreachable.notice().as_str())
        );
        assert!(store.path().exists(), "the token is still there");
        assert_eq!(store.load().expect("load").token, Some(token));
    }

    /// A token that is real but could not be written is still a working session.
    /// Throwing it away would break the app over a full disk.
    #[test]
    fn a_refresh_that_cannot_be_saved_still_works_this_session() {
        let (_home, store) = sandbox("unsaved");
        let endpoint = MockEndpoint::granting();
        let client = SpotifyEndpoint::at(&endpoint.url);
        let token = Token::from_response(&granted(), at(0)).expect("token");
        // A directory where the file goes: the rename cannot land, with no
        // permissions needed to make it fail.
        fs::create_dir_all(store.path()).expect("dir in the way");

        let auth = Session::new(&client, &store, CLIENT).access(&token, at(7200));

        let refreshed = auth.token().expect("the token is still usable");
        assert_eq!(refreshed.access_token(), ACCESS);
        assert!(
            matches!(auth.outcome, AuthOutcome::NotSaved(_)),
            "{:?}",
            auth.outcome
        );
        let notice = auth.notice().expect("something to say");
        assert!(notice.contains("this session is fine"), "{notice}");
    }

    // ---------------------------------------------------------------- notices

    /// One line per error, no newlines, and no token material in any of them --
    /// including the two that are built from a request that carried one. Asserted
    /// by searching the notice for the secret, because "we do not log tokens" is
    /// only a claim until something checks it.
    #[test]
    fn every_error_notice_is_one_line_and_holds_no_token() {
        let errors = [
            AuthError::Bind,
            AuthError::Browser,
            AuthError::Timeout { seconds: 300 },
            AuthError::NoRedirect,
            AuthError::StateMismatch,
            AuthError::Denied {
                reason: "access_denied".into(),
            },
            AuthError::NoCode,
            AuthError::Refused,
            AuthError::Unreachable,
            AuthError::Malformed,
            AuthError::RefreshRejected,
            AuthError::Store(StoreError::Write {
                path: PathBuf::from("/tmp/trak/token.json"),
            }),
        ];
        for error in &errors {
            let notice = error.notice();
            assert!(!notice.is_empty(), "{error:?}");
            assert!(!notice.contains('\n'), "{notice:?} is more than one line");
            assert!(notice.starts_with("trak: "), "{notice:?}");
            for secret in [ACCESS, REFRESH, CODE, RFC_VERIFIER] {
                assert!(!notice.contains(secret), "{notice:?} leaked a secret");
            }
        }
    }

    /// The structs that hold secrets redact rather than derive, so a `{:?}` in a
    /// log or a panic message is safe by construction.
    #[test]
    fn the_request_types_redact_what_they_carry() {
        let exchange = CodeExchange {
            code: CODE,
            redirect_uri: "http://127.0.0.1:1234",
            code_verifier: RFC_VERIFIER,
            client_id: CLIENT,
        };
        let debug = format!("{exchange:?}");
        assert!(!debug.contains(CODE), "{debug:?}");
        assert!(!debug.contains(RFC_VERIFIER), "{debug:?}");
        assert!(
            debug.contains("127.0.0.1:1234"),
            "the redirect is not secret"
        );

        let refresh = RefreshRequest {
            refresh_token: REFRESH,
            client_id: CLIENT,
        };
        assert!(!format!("{refresh:?}").contains(REFRESH));
    }

    /// The states the TUI has to tell apart, and the reason they are not one
    /// error: a spent refresh token is a normal end of a six-month connection, a
    /// 5xx is not, and neither is a file that could not be written.
    #[test]
    fn a_spent_token_and_a_transient_fault_are_different_states() {
        for dead in [
            AuthError::RefreshRejected,
            AuthError::Denied { reason: "x".into() },
            AuthError::Timeout { seconds: 1 },
            AuthError::StateMismatch,
            AuthError::NoCode,
            AuthError::NoRedirect,
            AuthError::Refused,
            AuthError::Browser,
        ] {
            assert!(dead.needs_relogin(), "{dead:?} is answered with a login");
        }
        for transient in [
            AuthError::Unreachable,
            AuthError::Malformed,
            AuthError::Bind,
        ] {
            assert!(!transient.needs_relogin(), "{transient:?} is worth a retry");
        }
    }

    /// The response parser, without a socket: a token, and everything that is not
    /// one.
    #[test]
    fn only_json_that_is_a_token_parses() {
        assert_eq!(
            parse_response(&granted_json())
                .expect("a token")
                .access_token,
            ACCESS
        );
        // A document that is JSON but not a token is caught by
        // `Token::from_response`, not here: this is the wire shape's parser.
        for body in ["", "not json", "{}", "[]", r#"{"access_token":1}"#] {
            assert_eq!(
                parse_response(body).expect_err("not a token"),
                AuthError::Malformed
            );
        }
    }

    /// A token response large enough to be something else is refused before it is
    /// parsed.
    #[test]
    fn an_oversized_token_response_is_refused() {
        let body = format!(
            r#"{{"access_token":"{}"}}"#,
            "a".repeat(MAX_BYTES as usize + 1)
        );
        assert_eq!(parse_response(&body), Err(AuthError::Malformed));
    }

    /// The percent codec, in both directions, against the characters that actually
    /// appear in this flow: a `:` and `/` in a redirect URI, a space in a scope
    /// list, a `+` in a code.
    #[test]
    fn the_percent_codec_round_trips() {
        for value in [
            "http://127.0.0.1:53219",
            "user-library-read playlist-read-private",
            "AQD8tN+4x9_example",
            "a&b=c#d",
            "ünïcode",
        ] {
            assert_eq!(percent_decode(&percent_encode(value)), value, "{value}");
        }
        assert_eq!(
            percent_encode("http://127.0.0.1:1"),
            "http%3A%2F%2F127.0.0.1%3A1"
        );
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(
            percent_decode("a%2"),
            "a%2",
            "a half escape is kept as it came"
        );
        assert_eq!(percent_decode("a%zz"), "a%zz");
    }

    /// The query parser has to cope with a target with no query at all, which is
    /// what a bare `GET /` looks like.
    #[test]
    fn a_target_with_no_query_has_no_parameters() {
        assert!(query_params("/").is_empty());
        assert!(query_params("/?code=1&state=2").len() == 2);
        assert!(query_params("/?flag").contains(&("flag".to_string(), String::new())));
    }

    /// The `error` filter, on its own: bounded, and letters and underscores only.
    #[test]
    fn a_hostile_error_value_is_reduced_to_something_that_is_not_a_secret() {
        assert_eq!(error_code("access_denied"), "access_denied");
        assert_eq!(error_code(&"a".repeat(200)), "a".repeat(32));
        assert_eq!(error_code(ACCESS), "BQAAtokenaccessvalue", "no separators");
        assert_eq!(error_code(""), "unknown");
    }

    /// The response body the mock serves, and the request types the tests build
    /// with it, come from the same shape.
    fn granted() -> TokenResponse {
        TokenResponse {
            access_token: ACCESS.to_string(),
            refresh_token: Some(REFRESH.to_string()),
            expires_in: Some(3600),
            scope: Some("user-library-read playlist-read-private".to_string()),
        }
    }

    fn granted_json() -> String {
        format!(
            r#"{{"access_token":"{ACCESS}","token_type":"Bearer","expires_in":3600,"refresh_token":"{REFRESH}"}}"#
        )
    }
}
