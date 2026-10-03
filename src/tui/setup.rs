//! The guided Spotify setup (TODO 7.3): the one screen that takes a user from
//! "no Client ID" to "logged in".
//!
//! It is a panel inside the settings screen (`s` opens it), so `trak config`
//! and the TUI's `,` both have it and neither needs a second keyboard owner.
//!
//! The split is the same as everywhere else in trak. [`Setup`] is pure state and
//! [`Setup::handle`] is the key table: it never opens a browser, never writes a
//! file and never touches the network, it only says what should happen as an
//! [`Effect`]. `loop_.rs` is the only thing that carries those out, which is
//! what lets every step here be tested without Spotify, a browser or a socket.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};

use crate::tui::app::App;
use crate::tui::theme::Theme;
use crate::web::auth::REGISTERED_REDIRECT_URI;

/// Where an app is created. An `https:` page, never `spotify:` -- opening that
/// would launch Spotify, which COMPAT rule 2 forbids as a side effect.
pub const DASHBOARD_URL: &str = "https://developer.spotify.com/dashboard";

/// What trak asks Spotify for, one scope per feature in `docs/WEB-API.md` §3.
/// Asking for less than a feature needs turns into a 403 on a key press; asking
/// for more costs the user a longer consent screen, so it is exactly this list.
pub const SCOPES: &[&str] = &[
    "user-read-currently-playing",
    "user-read-playback-state",
    "user-modify-playback-state",
    "user-library-read",
    "user-library-modify",
    "user-follow-read",
    "user-read-recently-played",
    "playlist-read-private",
    "playlist-modify-public",
    "playlist-modify-private",
];

/// A Client ID is 32 hex digits. Checked so a pasted line of the wrong thing
/// (the secret, the app name, the redirect URI) is caught on the spot rather
/// than as a cryptic "invalid client" page in the browser a step later.
const CLIENT_ID_LEN: usize = 32;

/// The four steps, in the order they are done.
const STEPS: [&str; 4] = [
    "Make an app on Spotify's developer site",
    "Add the redirect URI and save",
    "Paste the Client ID trak needs",
    "Log in with Spotify",
];

/// Why a pasted Client ID was refused. One line each, because it is shown under
/// the input while the user is still looking at it.
pub fn client_id_problem(raw: &str) -> Option<String> {
    clean_client_id(raw).err()
}

/// The ID a paste means: trimmed, unquoted, lowercased. `Err` is the sentence to
/// show.
pub fn clean_client_id(raw: &str) -> Result<String, String> {
    let id = raw
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim()
        .to_ascii_lowercase();
    if id.is_empty() {
        return Err("paste the Client ID from your app's Settings page".into());
    }
    if !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(
            "a Client ID is only 0-9 and a-f -- is this the app name or the secret?".into(),
        );
    }
    if id.len() != CLIENT_ID_LEN {
        return Err(format!(
            "a Client ID is {CLIENT_ID_LEN} characters; this is {}",
            id.len()
        ));
    }
    Ok(id)
}

/// Where the browser login is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LoginState {
    #[default]
    Idle,
    /// The browser is open and the loopback listener is waiting for it.
    Waiting,
    /// The last attempt did not finish. The text is an `AuthError` notice, which
    /// never carries a credential.
    Failed(String),
    Done,
}

/// Something the loop has to do on the panel's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Open(String),
    Copy(String),
    /// A valid ID was accepted. The glue puts it in the config and saves it.
    ClientId(String),
    /// The config file should be written now, so a login that outlives the
    /// session still has its ID next time.
    SaveConfig,
    Login(String),
    Logout,
}

/// The panel's state. `open` false means the settings checklist has the keys.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Setup {
    pub open: bool,
    /// 0-3, the step the cursor is on.
    pub step: usize,
    /// The Client ID being typed, if the input has focus.
    pub typing: Option<String>,
    /// Why the last paste was refused.
    pub error: Option<String>,
    pub login: LoginState,
    /// A one-line result of the last action ("copied", "logged out").
    pub notice: Option<String>,
    /// Effects the loop has not run yet.
    pub pending: Vec<Effect>,
}

impl Setup {
    /// Open on the step that is next for this user: a user with an ID has done
    /// steps 1-3 already and is here to log in or out.
    pub fn open(&mut self, client_id: &str, connected: bool) {
        *self = Setup {
            open: true,
            step: if client_id.is_empty() { 0 } else { 3 },
            login: if connected {
                LoginState::Done
            } else {
                LoginState::Idle
            },
            ..Setup::default()
        };
    }

    /// Handle one key. Chars arrive the way `loop_.rs`'s `char_for` folds them:
    /// arrows as `hjkl`, enter as `\n`, backspace as `\x7f`.
    pub fn handle(&mut self, c: char, client_id: &str, connected: bool) {
        if self.typing.is_some() {
            self.type_key(c);
            return;
        }
        self.notice = None;
        match c {
            'j' | '\t' => self.step = (self.step + 1).min(STEPS.len() - 1),
            'k' => self.step = self.step.saturating_sub(1),
            'c' => self
                .pending
                .push(Effect::Copy(REGISTERED_REDIRECT_URI.into())),
            'x' if connected => self.pending.push(Effect::Logout),
            '\n' => self.act(client_id),
            'q' | 'Q' | '\x1b' => self.open = false,
            _ => {}
        }
    }

    fn act(&mut self, client_id: &str) {
        match self.step {
            0 => self.pending.push(Effect::Open(DASHBOARD_URL.into())),
            1 => self
                .pending
                .push(Effect::Copy(REGISTERED_REDIRECT_URI.into())),
            2 => {
                self.error = None;
                self.typing = Some(client_id.to_string());
            }
            _ => {
                // A second login while one is waiting would bind a second port
                // and open a second tab for the same person.
                if self.login == LoginState::Waiting {
                    return;
                }
                match clean_client_id(client_id) {
                    Ok(id) => {
                        self.login = LoginState::Waiting;
                        self.pending.push(Effect::Login(id));
                    }
                    Err(_) => {
                        self.step = 2;
                        self.notice = Some("add the Client ID first (step 3)".into());
                    }
                }
            }
        }
    }

    fn type_key(&mut self, c: char) {
        match c {
            '\n' => {
                let line = self.typing.clone().unwrap_or_default();
                match clean_client_id(&line) {
                    Ok(id) => {
                        self.typing = None;
                        self.error = None;
                        self.step = 3;
                        self.pending.push(Effect::ClientId(id));
                    }
                    // Stays in the input: the paste is still there to fix.
                    Err(why) => self.error = Some(why),
                }
            }
            '\x1b' => {
                self.typing = None;
                self.error = None;
            }
            '\x08' | '\x7f' => {
                if let Some(line) = &mut self.typing {
                    line.pop();
                }
            }
            c if crate::tui::app::is_typed(c) => {
                if let Some(line) = &mut self.typing {
                    line.push(c);
                }
            }
            _ => {}
        }
    }

    /// The browser login ended.
    pub fn login_finished(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.login = LoginState::Done;
                self.notice = Some("connected -- search, playlists and the queue are on".into());
            }
            Err(why) => self.login = LoginState::Failed(why),
        }
    }

    /// The token is gone.
    pub fn logged_out(&mut self) {
        self.login = LoginState::Idle;
        self.notice = Some("logged out -- the token file is deleted".into());
    }
}

/// Route a key from the settings screen to the panel and apply what is pure.
pub fn key(app: &mut App, c: char) {
    let id = app.config.spotify.client_id.clone();
    let connected = app.web.connection.connected();
    app.setup.handle(c, &id, connected);
    // `ClientId` is the one effect that is only a state change, so it is applied
    // here and the file write is queued behind it for the loop.
    let mut rest = Vec::new();
    for effect in std::mem::take(&mut app.setup.pending) {
        match effect {
            Effect::ClientId(id) => {
                app.config.spotify.client_id = id;
                app.config_dirty = true;
                rest.push(Effect::SaveConfig);
            }
            other => rest.push(other),
        }
    }
    app.setup.pending = rest;
}

/// Draw the panel over `area`.
pub fn render(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    // The same floor the checklist has: below it the steps cannot be read, and
    // a word about the terminal is better than something torn (SPEC §3).
    if area.width < 30 || area.height < 8 {
        f.render_widget(
            Paragraph::new("terminal too small — resize")
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let s = &app.setup;
    let width = area.width.min(68);
    let panel = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        width,
        ..area
    };
    f.render_widget(Clear, panel);
    let block = Block::bordered()
        .title(" Connect Spotify ")
        .border_type(theme.border.to_ratatui().unwrap_or(BorderType::Rounded))
        .border_style(theme.accent_style());
    let inner = block.inner(panel);
    f.render_widget(block, panel);
    let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
    let compact = parts[0].height < 13;

    let dim = Theme::dim();
    let bold = Style::default().add_modifier(Modifier::BOLD);
    // One block of lines per step, so the view can be scrolled to the step the
    // cursor is on. Without that a terminal too short for all four explanations
    // simply cut the last one off, and step 4 -- the login -- was the one that
    // disappeared.
    let mut blocks: Vec<Vec<Line<'static>>> = Vec::with_capacity(STEPS.len());
    // One column is kept clear on the right as well, so no word touches either
    // border.
    let room = usize::from(parts[0].width).saturating_sub(EXPLAIN_INDENT.len() + 1);
    for (i, title) in STEPS.iter().enumerate() {
        let here = i == s.step;
        let mark = if here { "▸" } else { " " };
        let style = if here {
            theme.accent_style().patch(bold)
        } else {
            Style::default()
        };
        let mut block: Vec<Line<'static>> = Vec::new();
        for (n, row) in word_wrap(title, room).into_iter().enumerate() {
            let head = if n == 0 {
                format!(" {mark} {}  ", i + 1)
            } else {
                EXPLAIN_INDENT.to_string()
            };
            block.push(Line::from(Span::styled(format!("{head}{row}"), style)));
        }
        // Every step explains itself; in a short terminal only the one the cursor
        // is on does, so all four steps still fit and the screen is never torn.
        // In a short terminal every step keeps its title and the step the cursor
        // is on keeps its first line -- the one that says what to press -- so the
        // shape of all four steps is still on screen. `j` scrolls to the rest.
        let texts = explain(i, app);
        for text in texts.iter().take(if compact {
            usize::from(here)
        } else {
            usize::MAX
        }) {
            for row in word_wrap(text, room) {
                block.push(Line::from(Span::styled(
                    format!("{EXPLAIN_INDENT}{row}"),
                    dim,
                )));
            }
        }
        if !compact {
            block.push(Line::default());
        }
        blocks.push(block);
    }

    // Scroll so the current step's last line is on screen. The cursor is always
    // visible at any terminal size, which is the whole point of a wizard.
    let height = parts[0].height as usize;
    let total: usize = blocks.iter().map(Vec::len).sum();
    let upto = blocks[..=s.step.min(blocks.len() - 1)]
        .iter()
        .map(Vec::len)
        .sum::<usize>();
    let scroll = upto.saturating_sub(height);
    let lines: Vec<Line<'static>> = blocks
        .into_iter()
        .flatten()
        .skip(scroll)
        .take(if total > height { height } else { total })
        .collect();
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), parts[0]);

    let footer = if let Some(why) = &s.error {
        Line::from(Span::styled(
            format!(" {why} "),
            Style::default().fg(Color::Yellow),
        ))
    } else if let Some(n) = &s.notice {
        Line::from(Span::styled(format!(" {n} "), theme.accent_style()))
    } else if s.typing.is_some() {
        Line::from(Span::styled(" enter saves it, esc cancels ", dim))
    } else {
        Line::from(Span::styled(
            " enter do this step · c copy the URI · x log out · j/k move · q back ",
            dim,
        ))
    };
    f.render_widget(Paragraph::new(footer).alignment(Alignment::Left), parts[1]);
}

/// Where an explanation starts, under its step's title.
const EXPLAIN_INDENT: &str = "      ";

/// Wrap at spaces to a display width, so a continued explanation keeps its
/// indent. Left to `Paragraph`'s own wrap, the continuation started flush
/// against the panel's border, and in cmux the border cells on exactly those
/// rows went missing (seen 2026-10-01). A word wider than the row is cut by
/// width rather than allowed to run past it.
fn word_wrap(text: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthStr;

    let width = width.max(1);
    let mut rows: Vec<String> = Vec::new();
    let mut row = String::new();
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        let needed = if row.is_empty() {
            word.width()
        } else {
            row.width() + 1 + word.width()
        };
        if needed <= width {
            if !row.is_empty() {
                row.push(' ');
            }
            row.push_str(word);
            continue;
        }
        if !row.is_empty() {
            rows.push(std::mem::take(&mut row));
        }
        let mut pieces = crate::tui::render::wrap_by_width(word, width);
        row = pieces.pop().unwrap_or_default();
        rows.extend(pieces);
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    rows
}

/// The lines under one step. Plain strings so a test can read them.
///
/// Written for somebody who has never made a Spotify app before: each step says
/// what to click, what to type, and what "it worked" looks like. The words
/// "redirect URI", "Client ID" and "Web API" are the dashboard's own, so the
/// instructions and the page use the same words -- but nothing is left to
/// inference (owner, 2026-10-02).
pub fn explain(step: usize, app: &App) -> Vec<String> {
    let s = &app.setup;
    match step {
        0 => vec![
            "press enter -- your browser opens the Spotify developer site.".into(),
            "Log in with your Spotify account if it asks, then press \"Create app\".".into(),
        ],
        1 => vec![
            format!("paste this into \"Redirect URI\": {REGISTERED_REDIRECT_URI}"),
            "Any name and description will do. Never use \"localhost\".".into(),
            "Tick \"Web API\", press \"Save\", and this page is done.".into(),
        ],
        2 => {
            let shown = match &s.typing {
                Some(line) => format!("{line}▏"),
                None if app.config.spotify.client_id.is_empty() => {
                    "press enter, then paste it here".to_string()
                }
                None => app.config.spotify.client_id.clone(),
            };
            vec![
                shown,
                "It is on the app's \"Settings\" page: 32 letters and numbers.".into(),
                "press enter to save it, esc to cancel.".into(),
            ]
        }
        _ => vec![match &s.login {
            LoginState::Idle => "press enter -- your browser opens to approve trak.".into(),
            LoginState::Waiting => "Your browser is open. Click \"Allow\", then come back.".into(),
            LoginState::Failed(why) => format!("{why} -- press enter to try again."),
            LoginState::Done => {
                "That is everything -- search, playlists and your library work now.".into()
            }
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    fn open() -> Setup {
        let mut s = Setup::default();
        s.open("", false);
        s
    }

    fn drain(s: &mut Setup) -> Vec<Effect> {
        std::mem::take(&mut s.pending)
    }

    #[test]
    fn a_client_id_is_32_hex_digits() {
        assert_eq!(clean_client_id(ID).as_deref(), Ok(ID));
        assert_eq!(
            clean_client_id(&format!("  \"{}\"\n", ID.to_uppercase())).as_deref(),
            Ok(ID)
        );
        assert!(clean_client_id("").is_err());
        assert!(clean_client_id("abc").unwrap_err().contains("3"));
        assert!(
            clean_client_id(&"z".repeat(32))
                .unwrap_err()
                .contains("0-9")
        );
        assert_eq!(client_id_problem(ID), None);
    }

    #[test]
    fn it_opens_on_the_step_that_is_next() {
        let mut s = Setup::default();
        s.open("", false);
        assert_eq!((s.open, s.step), (true, 0));
        s.open(ID, false);
        assert_eq!(s.step, 3);
        s.open(ID, true);
        assert_eq!(s.login, LoginState::Done);
    }

    #[test]
    fn step_one_opens_the_dashboard_and_step_two_copies_the_uri() {
        let mut s = open();
        s.handle('\n', "", false);
        assert_eq!(drain(&mut s), [Effect::Open(DASHBOARD_URL.into())]);
        s.handle('j', "", false);
        s.handle('\n', "", false);
        assert_eq!(
            drain(&mut s),
            [Effect::Copy(REGISTERED_REDIRECT_URI.into())]
        );
        s.handle('c', "", false);
        assert_eq!(
            drain(&mut s),
            [Effect::Copy(REGISTERED_REDIRECT_URI.into())]
        );
    }

    #[test]
    /// What the guided setup sends the user to, and what it tells them to type,
    /// are both pinned: the dashboard rejects anything else, so a change here is
    /// a change to what the instructions have to say (measured 2026-10-02).
    fn the_dashboard_is_https_and_the_uri_is_the_form_the_dashboard_accepts() {
        assert!(DASHBOARD_URL.starts_with("https://"));
        assert_eq!(REGISTERED_REDIRECT_URI, "http://127.0.0.1:8888/callback");
        assert!(!REGISTERED_REDIRECT_URI.contains("localhost"));
    }

    #[test]
    fn a_bad_paste_stays_in_the_input_with_the_reason() {
        let mut s = open();
        s.step = 2;
        s.handle('\n', "", false);
        for c in "nope".chars() {
            s.handle(c, "", false);
        }
        s.handle('\n', "", false);
        assert!(s.typing.is_some());
        assert!(s.error.is_some());
        assert!(drain(&mut s).is_empty());
        // Fixing it clears the complaint and moves on to the login.
        s.handle('\x1b', "", false);
        assert_eq!((s.typing.clone(), s.error.clone()), (None, None));
    }

    #[test]
    fn a_good_paste_is_accepted_and_moves_to_login() {
        let mut s = open();
        s.step = 2;
        s.handle('\n', "", false);
        for c in ID.chars() {
            s.handle(c, "", false);
        }
        s.handle('\n', "", false);
        assert_eq!(drain(&mut s), [Effect::ClientId(ID.into())]);
        assert_eq!((s.typing.clone(), s.step), (None, 3));
    }

    #[test]
    fn backspace_erases_and_control_characters_are_not_typed() {
        let mut s = open();
        s.step = 2;
        s.handle('\n', "ab", false);
        s.handle('\x7f', "", false);
        s.handle('\u{1}', "", false);
        assert_eq!(s.typing.as_deref(), Some("a"));
    }

    #[test]
    fn login_needs_an_id_and_only_runs_once_at_a_time() {
        let mut s = open();
        s.step = 3;
        s.handle('\n', "", false);
        assert!(drain(&mut s).is_empty());
        assert_eq!(s.step, 2);
        s.step = 3;
        s.handle('\n', ID, false);
        assert_eq!(drain(&mut s), [Effect::Login(ID.into())]);
        assert_eq!(s.login, LoginState::Waiting);
        s.handle('\n', ID, false);
        assert!(drain(&mut s).is_empty(), "a second login must not start");
    }

    #[test]
    fn a_failed_login_can_be_retried() {
        let mut s = open();
        s.step = 3;
        s.handle('\n', ID, false);
        drain(&mut s);
        s.login_finished(Err(
            "trak: the Spotify login was not completed (access_denied)".into(),
        ));
        assert!(matches!(s.login, LoginState::Failed(_)));
        s.handle('\n', ID, false);
        assert_eq!(drain(&mut s), [Effect::Login(ID.into())]);
        s.login_finished(Ok(()));
        assert_eq!(s.login, LoginState::Done);
    }

    #[test]
    fn logout_is_offered_only_when_connected() {
        let mut s = open();
        s.handle('x', ID, false);
        assert!(drain(&mut s).is_empty());
        s.handle('x', ID, true);
        assert_eq!(drain(&mut s), [Effect::Logout]);
    }

    #[test]
    fn q_and_escape_close_the_panel() {
        let mut s = open();
        s.handle('q', "", false);
        assert!(!s.open);
        s.open = true;
        s.handle('\x1b', "", false);
        assert!(!s.open);
    }

    #[test]
    fn the_scopes_cover_every_feature_and_nothing_extra() {
        for needed in [
            "user-library-modify",
            "user-modify-playback-state",
            "playlist-modify-private",
        ] {
            assert!(SCOPES.contains(&needed), "{needed}");
        }
        let mut sorted = SCOPES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), SCOPES.len(), "a scope is listed twice");
    }

    #[test]
    fn glue_stores_the_id_in_the_config_and_queues_the_save() {
        let mut app = App::new();
        app.setup.open("", false);
        app.setup.step = 2;
        key(&mut app, '\n');
        for c in ID.chars() {
            key(&mut app, c);
        }
        key(&mut app, '\n');
        assert_eq!(app.config.spotify.client_id, ID);
        assert!(app.config_dirty);
        assert_eq!(app.setup.pending, [Effect::SaveConfig]);
    }

    fn drawn(width: u16, height: u16, app: &App) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut t = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = Theme::new(
            crate::tui::theme::Accent::Green,
            crate::tui::theme::Border::Rounded,
        );
        t.draw(|f| render(f, f.area(), app, &theme)).unwrap();
        let buf = t.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn explanations_wrap_at_words_and_never_reach_the_border() {
        let mut app = App::new();
        app.setup.open("", false);
        for width in [40u16, 52, 70, 100] {
            let text = drawn(width, 30, &app);
            for row in text.lines() {
                let inner: String = row.chars().skip(1).collect();
                // Every row inside the panel starts with the border, then a
                // space: nothing is drawn flush against it.
                if row.starts_with('│') {
                    assert!(inner.starts_with(' '), "{width}: {row}\n{text}");
                }
            }
            // The URI is never split across rows, so it can be read and copied.
            assert!(text.contains(REGISTERED_REDIRECT_URI), "{width}\n{text}");
        }
    }

    #[test]
    fn word_wrap_keeps_words_whole_and_cuts_only_a_word_too_wide() {
        assert_eq!(word_wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(word_wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(word_wrap("", 10), [""]);
        assert_eq!(word_wrap("a  b", 10), ["a b"]);
        for row in word_wrap("enter to type or paste it (the 32 characters)", 9) {
            assert!(
                unicode_width::UnicodeWidthStr::width(row.as_str()) <= 9,
                "{row}"
            );
        }
    }

    #[test]
    fn every_step_has_an_explanation_on_screen_when_there_is_room() {
        let mut app = App::new();
        app.setup.open("", false);
        let text = drawn(70, 40, &app);
        for needle in [
            "Create app",
            REGISTERED_REDIRECT_URI,
            "paste it here",
            "approve trak",
        ] {
            assert!(text.contains(needle), "{needle}\n{text}");
        }
    }

    #[test]
    fn a_short_terminal_still_draws_all_four_steps_and_the_current_explanation() {
        let mut app = App::new();
        app.setup.open("", false);
        let text = drawn(60, 10, &app);
        for n in ["1 ", "2 ", "3 ", "4 "] {
            assert!(text.contains(n), "{n}\n{text}");
        }
        assert!(text.contains("press enter"), "{text}");
    }

    #[test]
    fn no_size_panics_the_panel_and_a_tiny_one_says_so() {
        let mut app = App::new();
        app.setup.open("", false);
        for (w, h) in [(1, 1), (10, 3), (29, 20), (60, 7), (60, 8), (200, 60)] {
            drawn(w, h, &app);
        }
        assert!(drawn(20, 5, &app).contains("too small"));
    }

    #[test]
    fn a_failed_login_says_why_and_how_to_retry() {
        let mut app = App::new();
        app.setup.open(ID, false);
        app.setup.login = LoginState::Failed("trak: Spotify refused the login".into());
        let text = drawn(70, 24, &app);
        // The step the cursor is on is scrolled into view at any size, so a
        // failure is readable on a terminal too short for all four steps.
        assert!(
            text.contains("refused the login -- press enter to try"),
            "{text}"
        );
        assert!(text.contains("4  Log in with Spotify"), "{text}");
    }
}
