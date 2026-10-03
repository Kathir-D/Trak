//! The settings screen: a grouped checklist of every key SPEC §8 defines
//! (TODO 5.2).
//!
//! One screen with two callers. `,` inside the TUI hands [`render`] the
//! dashboard and `trak config` hands it the whole frame; the screen picks its
//! own panel inside whatever rectangle it is given, so both get the same
//! checklist. Nothing here reads a track, the artwork or the poll, which is what
//! lets the standalone `trak config` run against a bare [`App::new`] with no
//! Spotify and no event loop.
//!
//! A change is written straight into `app.settings` and nothing else, so the
//! running TUI has it on the next frame. That is the live preview SPEC §8
//! promises, and it is free because there is no second copy of the settings to
//! reconcile -- the one thing this screen cannot do itself is rebuild the
//! `Theme`, which belongs to the loop.
//!
//! # The line being typed
//!
//! `,` on `client_id` opens a one-line editor, and that line is the only state
//! here that does not fit on [`App`]. It is a thread-local, not a `static mut`,
//! so nothing can race it and nothing can read it while it is half written. The
//! price is that [`open`], [`key`] and [`render`] are thread-affine together,
//! which they are: the event loop owns the keyboard and draws the frame.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};

use crate::config::{ArtProtocol, VisualizerSource, tab_name};
use crate::tui::app::{App, DisplayMode, Tab, Toast, VisualizerStyle, VolumeControl};
use crate::tui::theme::{Accent, Border, Theme};

/// The widest the panel is ever drawn.
///
/// Two columns of short text do not need more, and a settings screen stretched
/// across 200 columns of terminal is a mostly-blank box with a checklist down
/// one side of it. It is also the width every key hint has to fit at, which is
/// what stops a narrower panel from quietly dropping the one that matters.
const MAX_WIDTH: u16 = 68;

/// How wide the key column is. The longest label is `art_protocol` (12) and
/// `volume_step` (11), so 16 leaves the values a column of their own at any size
/// the panel is drawn at.
const LABEL_WIDTH: usize = 16;

/// Below this the checklist cannot be read, and SPEC §3 asks for a word about
/// the terminal rather than something torn.
const MIN_WIDTH: u16 = 30;
const MIN_HEIGHT: u16 = 8;

/// How many lines of what follows the cursor the view keeps on screen. Two is
/// enough to see what `j` is about to land on without the list scrolling on
/// every other keypress.
const LOOKAHEAD: usize = 2;

/// The group the guided Spotify setup belongs to, and what its heading says.
const SPOTIFY_GROUP: &str = "Spotify API";
const HINT_SETUP: &str = "  ·  s: guided setup";

thread_local! {
    /// The line being typed into, or `None` when the screen is not in text mode.
    ///
    /// Thread-local rather than a global `Mutex` and the reason is the tests: the
    /// crate's unit tests run in parallel threads, and one shared buffer would
    /// mean a test that opened a line editor decided what another test's screen
    /// drew. The event loop, the renderer and the key handler are all on its own
    /// thread, so there is nothing here to serialise -- but that does make
    /// [`open`], [`key`] and [`render`] thread-affine, and they are.
    static TYPING: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Enter the screen. Called when `,` is pressed.
///
/// The cursor starts on the first setting, and any half-typed line is dropped:
/// the only way to reach this function is through a `,`, and a `,` means "show
/// me the settings", not "carry on with what I was writing".
pub fn open(app: &mut App) {
    app.settings_open = true;
    app.settings_cursor = 0;
    forget_typing();
}

/// Handle one key. The screen owns the keyboard while it is open.
///
/// Every key the loop sends is a char, with `↓`/`↑` already folded onto `j`/`k`
/// (`loop_.rs`'s `char_for`); `←`/`→` arrive as sentinels and are folded onto
/// `h`/`l` here, so one match covers both.
pub fn key(app: &mut App, c: char) {
    // On this screen `←`/`→` change a value, as `h`/`l` do; it has no tabs.
    let c = match c {
        crate::tui::app::ARROW_LEFT if !is_typing() && !app.setup.open => 'h',
        crate::tui::app::ARROW_RIGHT if !is_typing() && !app.setup.open => 'l',
        c => c,
    };
    if app.setup.open {
        crate::tui::setup::key(app, c);
        return;
    }
    if is_typing() {
        type_key(app, c);
        return;
    }
    match c {
        'j' => move_cursor(app, 1),
        'k' => move_cursor(app, -1),
        ' ' => toggle(app),
        'h' => change(app, Way::Left),
        'l' => change(app, Way::Right),
        '\n' => begin_typing(app),
        's' | 'S' => {
            let connected = app.web.connection.connected();
            app.setup.open(&app.config.spotify.client_id, connected);
        }
        // `Q` as well as `q`, because that is what every other screen in trak
        // takes, and a user with caps lock on expects `q` to close this too.
        // `?` too, because `?` is what opened it from the dashboard.
        'q' | 'Q' | '\x1b' | '?' => save_and_close(app, &config_path()),
        _ => {}
    }
}

/// Draw the screen. The area is the caller's: the whole frame for `trak config`,
/// and a rectangle over the dashboard for `,`.
pub fn render(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if app.setup.open {
        crate::tui::setup::render(f, area, app, theme);
        return;
    }
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        f.render_widget(
            Paragraph::new("terminal too small — resize")
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    // Every key trak has is listed here too, since `?` opens this screen: in a
    // panel of its own beside the settings when the terminal is wide enough,
    // and under them otherwise.
    let panel = panel(area);
    let beside = keys_panel(area);
    if let Some(keys) = beside {
        f.render_widget(Clear, keys);
        f.render_widget(
            Paragraph::new(key_lines(theme)).block(
                Block::bordered()
                    .title(" Keys ")
                    .border_type(theme.border.to_ratatui().unwrap_or(BorderType::Rounded))
                    .border_style(theme.accent_style()),
            ),
            keys,
        );
    }
    let beside = beside.is_some();
    f.render_widget(Clear, panel);

    let mut block = Block::bordered()
        .title(" Settings ")
        // The same fallback `render.rs`'s panes use, so a `border = "none"`
        // setting cannot leave this panel with no edge to read against.
        .border_type(theme.border.to_ratatui().unwrap_or(BorderType::Rounded))
        .border_style(theme.accent_style());
    if panel.width >= 40 {
        block = block.title_top(
            Line::from(" changes apply live ")
                .right_aligned()
                .patch_style(theme.accent_style()),
        );
    }
    // The bottom border speaks only while a line is being typed: the keys do
    // something else then, and the rest of the time the hints row says what
    // they do.
    if is_typing() {
        block = block
            .title_bottom(Line::from(" enter saves it, esc cancels ").patch_style(Theme::dim()));
    }
    let inner = block.inner(panel);
    f.render_widget(block, panel);

    // One row is the key hints, or the notice when there is one to read. Both on
    // the same row so that a notice arriving does not shift the checklist.
    let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
    let (rows, footer) = (parts[0], parts[1]);
    if rows.height == 0 {
        return;
    }

    let items = items();
    let mut lines: Vec<Line<'static>> = items.iter().map(|i| line_for(i, app, theme)).collect();
    let cursor_line = line_of(cursor_of(app));
    let mut scroll = scroll_for(cursor_line, rows.height as usize, items.len());
    if !beside {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Keys",
            theme.accent_style().add_modifier(Modifier::BOLD),
        )));
        lines.extend(key_lines(theme));
        // On the last setting, show as much of the key list as fits while
        // keeping the cursor's row on screen: that is the only way down to it.
        if cursor_of(app) + 1 == row_count() {
            scroll = lines
                .len()
                .saturating_sub(rows.height as usize)
                .min(cursor_line)
                .max(scroll);
        }
    }
    f.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), rows);

    if let Some(at) = caret(app, rows, scroll) {
        f.set_cursor_position(at);
    }
    f.render_widget(footer_line(app, theme, footer.width as usize), footer);
}

// ---------------------------------------------------------------- the table

/// The screen, in the order it is drawn.
///
/// A group is a way of *finding* a setting, not a table in the file. `border`,
/// `accent` and `art_protocol` are `[display]` keys that read as "Theme" to
/// anybody who has not opened the file -- they are all about how the thing is
/// drawn rather than what is in it -- and `[lyrics] enabled` sits with the other
/// on/off display toggles because that is what it does. `control` is the one row
/// SPEC §8's example does not show: TODO 4.4 pins it in the file and TODO 5.1
/// records that SPEC wants the line added.
const GROUPS: &[(&str, &[Row])] = &[
    (
        "Display",
        &[
            ("art", Control::Toggle(Toggle::Art)),
            ("mode", Control::Choice(Choice::Mode)),
            ("progress", Control::Toggle(Toggle::Progress)),
            ("volume", Control::Toggle(Toggle::Volume)),
            ("popularity", Control::Toggle(Toggle::Popularity)),
            ("key_hints", Control::Toggle(Toggle::KeyHints)),
            ("clock", Control::Toggle(Toggle::Clock)),
            ("side_pane", Control::Toggle(Toggle::SidePane)),
            ("default_tab", Control::Choice(Choice::DefaultTab)),
            ("enabled", Control::Toggle(Toggle::Lyrics)),
        ],
    ),
    (
        "Theme",
        &[
            ("border", Control::Choice(Choice::Border)),
            ("accent", Control::Choice(Choice::Accent)),
            ("art_protocol", Control::Choice(Choice::ArtProtocol)),
        ],
    ),
    (
        "Visualizer",
        &[
            ("style", Control::Choice(Choice::VizStyle)),
            ("source", Control::Choice(Choice::VizSource)),
        ],
    ),
    (
        "Input",
        &[
            ("mouse", Control::Toggle(Toggle::Mouse)),
            ("volume_step", Control::Number(Stepper::VolumeStep)),
            ("control", Control::Choice(Choice::VolumeControl)),
        ],
    ),
    (
        "Notifications",
        &[("song_change", Control::Toggle(Toggle::SongChange))],
    ),
    (
        "Spotify API",
        &[(
            "client_id",
            Control::Text {
                get: client_id,
                set: set_client_id,
            },
        )],
    ),
];

/// One editable setting: the key the file writes it under, so the screen and
/// `config.toml` cannot drift apart in what they call it, and how it is edited.
///
/// A tuple rather than a struct because it is only ever destructured, and
/// because it is the shape rustfmt leaves as a table: one setting per line is
/// the point of the table.
type Row = (&'static str, Control);

/// What kind of editing a row takes. Every variant is a choice from a fixed set
/// or a line of text, which is what makes an unrenderable state impossible: there
/// is nowhere to type a value that the file cannot hold.
#[derive(Debug, Clone, Copy)]
enum Control {
    /// `space` flips it.
    Toggle(Toggle),
    /// `←`/`→` walk a set of choices, wrapping at both ends.
    Choice(Choice),
    /// `←`/`→` walk a sorted ladder of numbers, stopping at both ends.
    Number(Stepper),
    /// `enter` opens a line of text. The only one is `[spotify] client_id`,
    /// which lives on the config rather than on `Settings`, so both accessors
    /// take the whole app.
    Text {
        get: fn(&App) -> String,
        set: fn(&mut App, &str),
    },
}

/// The settings that are on or off.
///
/// One arm per field, so a row cannot be wired to the wrong setting: a rename on
/// `Settings` is a compile error here rather than a toggle that flips something
/// else.
#[derive(Debug, Clone, Copy)]
enum Toggle {
    Art,
    Progress,
    Volume,
    Popularity,
    KeyHints,
    Clock,
    SidePane,
    Lyrics,
    Mouse,
    SongChange,
}

impl Toggle {
    fn get(self, app: &App) -> bool {
        match self {
            Self::Art => app.settings.show_art,
            Self::Progress => app.settings.show_progress,
            Self::Volume => app.settings.show_volume,
            Self::Popularity => app.settings.show_popularity,
            Self::KeyHints => app.settings.show_key_hints,
            Self::Clock => app.settings.show_clock,
            Self::SidePane => app.settings.side_pane,
            Self::Lyrics => app.settings.lyrics,
            Self::Mouse => app.settings.mouse,
            Self::SongChange => app.settings.song_change_notification,
        }
    }

    fn set(self, app: &mut App, on: bool) {
        match self {
            Self::Art => app.settings.show_art = on,
            Self::Progress => app.settings.show_progress = on,
            Self::Volume => app.settings.show_volume = on,
            Self::Popularity => app.settings.show_popularity = on,
            Self::KeyHints => app.settings.show_key_hints = on,
            Self::Clock => app.settings.show_clock = on,
            Self::SidePane => app.settings.side_pane = on,
            Self::Lyrics => app.settings.lyrics = on,
            Self::Mouse => app.settings.mouse = on,
            Self::SongChange => app.settings.song_change_notification = on,
        }
    }
}

/// The settings with a fixed set of values.
#[derive(Debug, Clone, Copy)]
enum Choice {
    Mode,
    DefaultTab,
    Border,
    Accent,
    ArtProtocol,
    VizStyle,
    VizSource,
    VolumeControl,
}

impl Choice {
    /// The value in the words the file writes it under.
    fn text(self, app: &App) -> &'static str {
        match self {
            Self::Mode => mode_name(app.settings.display_mode),
            Self::DefaultTab => tab_name(app.settings.default_tab),
            Self::Border => app.settings.border.name(),
            Self::Accent => accent_name(app.settings.accent),
            Self::ArtProtocol => app.settings.art_protocol.label(),
            Self::VizStyle => app.settings.visualizer_style.label(),
            Self::VizSource => app.settings.visualizer_source.label(),
            Self::VolumeControl => app.settings.volume_control.label(),
        }
    }

    /// The next value `way`. The list is the type's own `ALL` where it has one,
    /// so a new variant appears on this screen without anybody remembering to
    /// add it, and in the order the type declares.
    fn step(self, app: &mut App, way: Way) {
        match self {
            Self::Mode => {
                let next = cycled(&MODES, app.settings.display_mode, way);
                app.settings.display_mode = next;
            }
            Self::DefaultTab => {
                let next = cycled(&Tab::ALL, app.settings.default_tab, way);
                app.settings.default_tab = next;
            }
            Self::Border => {
                let next = cycled(&Border::ALL, app.settings.border, way);
                app.settings.border = next;
            }
            Self::Accent => {
                let next = cycled(&ACCENTS, app.settings.accent, way);
                app.settings.accent = next;
            }
            Self::ArtProtocol => {
                let next = cycled(&ArtProtocol::ALL, app.settings.art_protocol, way);
                app.settings.art_protocol = next;
            }
            Self::VizStyle => {
                let next = cycled(&VisualizerStyle::ALL, app.settings.visualizer_style, way);
                app.settings.visualizer_style = next;
            }
            Self::VizSource => {
                let next = cycled(&VisualizerSource::ALL, app.settings.visualizer_source, way);
                app.settings.visualizer_source = next;
            }
            Self::VolumeControl => {
                let next = cycled(&VOLUME_CONTROLS, app.settings.volume_control, way);
                app.settings.volume_control = next;
            }
        }
    }
}

/// The settings that are a number, which `←`/`→` walk a ladder rather than
/// taking a typed value: a number typed into a field is the one thing on this
/// screen that could be a value nothing else can use.
#[derive(Debug, Clone, Copy)]
enum Stepper {
    VolumeStep,
}

impl Stepper {
    fn text(self, app: &App) -> String {
        match self {
            Self::VolumeStep => app.settings.volume_step.to_string(),
        }
    }

    /// The next rung up or down. The ladder is the range: nothing off the end of
    /// it can be produced, so a step stays in range however the file was edited.
    fn step(self, app: &mut App, way: Way) {
        match self {
            Self::VolumeStep => {
                let next = along(&VOLUME_STEPS, app.settings.volume_step, way);
                app.settings.volume_step = next;
            }
        }
    }
}

/// `[display] mode`, in the order `←`/`→` walk it.
const MODES: [DisplayMode; 2] = [DisplayMode::Art, DisplayMode::Visualizer];

/// `[display] accent`, in the order `←`/`→` walk it.
///
/// `Accent` has no `name()` the way `Border` has one, so the list and the words
/// are here. A test holds them to what `Accent::parse` accepts, which is the
/// only definition that matters: a word the file cannot read is not a setting.
const ACCENTS: [Accent; 3] = [Accent::Art, Accent::Green, Accent::Terminal];

/// `[volume] control`, in the order `←`/`→` walk it. Same reason as [`ACCENTS`].
const VOLUME_CONTROLS: [VolumeControl; 2] = [VolumeControl::Spotify, VolumeControl::System];

/// `[input] volume_step`, in points of volume. Every entry is inside 1..=100,
/// which is the whole of the volume scale, so no number of presses can produce a
/// step the meter cannot show.
const VOLUME_STEPS: [i16; 8] = [1, 2, 5, 10, 15, 20, 25, 50];

/// `mode`'s word in the file. `DisplayMode` carries a `parse` but no `name`, and
/// `config.rs` keeps its own copy private, so the screen needs its own. The
/// test that walks every word through `parse` is what makes the two the same.
fn mode_name(mode: DisplayMode) -> &'static str {
    match mode {
        DisplayMode::Art => "art",
        DisplayMode::Visualizer => "visualizer",
    }
}

/// `accent`'s word in the file, for the same reason as [`mode_name`].
fn accent_name(accent: Accent) -> &'static str {
    match accent {
        Accent::Art => "art",
        Accent::Green => "green",
        Accent::Terminal => "terminal",
    }
}

/// Which way a value steps. Named for the keys rather than for "up" and "down",
/// because the loop sends `h` for `←` and `l` for `→` and there is no other sense
/// of the two directions here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Way {
    Left,
    Right,
}

// ------------------------------------------------------------------ editing

/// Move the cursor, stopping at both ends.
fn move_cursor(app: &mut App, delta: isize) {
    let last = row_count().saturating_sub(1) as isize;
    let from = cursor_of(app) as isize;
    app.settings_cursor = (from + delta).clamp(0, last) as usize;
}

/// The cursor as a row index, clamped into the table.
///
/// Clamped rather than trusted because nothing else guarantees it is in range:
/// it is a plain `usize` on `App`, and a screen that indexes with it must not be
/// the thing that turns a stale value into a panic.
fn cursor_of(app: &App) -> usize {
    app.settings_cursor.min(row_count().saturating_sub(1))
}

/// The row the cursor is on.
fn row_at(app: &App) -> Option<&'static Row> {
    row_by_index(cursor_of(app))
}

/// The `n`th setting in screen order.
fn row_by_index(index: usize) -> Option<&'static Row> {
    let mut left = index;
    for (_, rows) in GROUPS {
        if left < rows.len() {
            return Some(&rows[left]);
        }
        left -= rows.len();
    }
    None
}

/// How many settings there are, which is one past the highest cursor.
fn row_count() -> usize {
    GROUPS.iter().map(|(_, rows)| rows.len()).sum()
}

/// The line the `n`th setting is drawn on.
///
/// Not `n`: the group headings are lines too and are not settings, so the
/// scroll and the caret have to count them. Getting this wrong puts the cursor
/// several lines above the row it is on, which is exactly the bug a scrolled
/// list shows up as.
fn line_of(row: usize) -> usize {
    let mut line = 0;
    let mut left = row;
    for (_, rows) in GROUPS {
        if left < rows.len() {
            return line + left;
        }
        // The heading, then the rows under it.
        line += 1 + rows.len();
        left -= rows.len();
    }
    line
}

/// The first line to draw, so the cursor is on screen.
///
/// Stateless on purpose: the only thing trak carries between frames is the
/// cursor on `App`, and a scroll that had to be remembered would be a second
/// thing to keep in step with it. The cursor is held near the bottom with
/// [`LOOKAHEAD`] lines of what follows, so `j` shows what is coming and `k`
/// walks back up through the list.
fn scroll_for(cursor: usize, height: usize, total: usize) -> usize {
    // Never past the end: the last page is the tail of the list rather than a
    // page of blanks.
    let most = total.saturating_sub(height);
    cursor
        .saturating_sub(height.saturating_sub(LOOKAHEAD))
        .min(most)
}

/// Flip the row under the cursor, if it is one that flips.
fn toggle(app: &mut App) {
    let Some((_, Control::Toggle(which))) = row_at(app) else {
        return;
    };
    which.set(app, !which.get(app));
    app.config_dirty = true;
}

/// Step the row under the cursor, if it is one that steps.
///
/// A toggle has nothing to step through and a text line is typed rather than
/// chosen, so `←`/`→` do nothing on them rather than guessing at something.
fn change(app: &mut App, way: Way) {
    let Some((_, control)) = row_at(app) else {
        return;
    };
    match control {
        Control::Choice(which) => which.step(app, way),
        Control::Number(which) => which.step(app, way),
        Control::Toggle(_) | Control::Text { .. } => return,
    }
    app.config_dirty = true;
}

/// Open the line editor for the text row under the cursor, seeded with what is
/// already there so an edit starts from the value rather than from nothing.
fn begin_typing(app: &mut App) {
    let Some((_, Control::Text { get, .. })) = row_at(app) else {
        return;
    };
    set_typing(Some(get(app)));
}

/// A key pressed while a line is being typed.
///
/// `esc` cancels and does not close: the checklist is still there and the value
/// is untouched, and a key that both cancelled *and* saved would be the one way
/// to lose work in a dialog.
fn type_key(app: &mut App, c: char) {
    match c {
        '\n' => commit_typing(app),
        '\x1b' => forget_typing(),
        // DEL and BS are the same key to a person and which one arrives is
        // crossterm's business. `loop_.rs`'s `char_for` has to send
        // `KeyCode::Backspace` through as one of these two, or the line editor
        // cannot erase at all.
        '\x08' | '\x7f' => map_typing(|line| {
            line.pop();
        }),
        c => map_typing(|line| {
            // A control character is a key the terminal wanted, not one the
            // person typed into a Client ID.
            if crate::tui::app::is_typed(c) {
                line.push(c);
            }
        }),
    }
}

/// Take the typed line and put it in the setting, then go back to the checklist.
///
/// The cursor cannot have moved while the line was being typed -- every key
/// inserts a character instead -- so the row being written is the row the line
/// was opened for.
fn commit_typing(app: &mut App) {
    let Some((_, Control::Text { set, .. })) = row_at(app) else {
        forget_typing();
        return;
    };
    let Some(line) = take_typing() else { return };
    // Trimmed, because a pasted ID usually arrives with a newline on the end of
    // it and a stray space in a config file is invisible until a request 401s.
    set(app, line.trim());
    app.config_dirty = true;
}

/// The value of `[spotify] client_id`, which is a string on the config and not
/// a field on `Settings` -- it is not a display choice, and giving `Settings` a
/// field for it would put a Web API credential in the thing the renderer reads
/// on every frame.
fn client_id(app: &App) -> String {
    app.config.spotify.client_id.clone()
}

fn set_client_id(app: &mut App, value: &str) {
    app.config.spotify.client_id = value.to_string();
}

// ------------------------------------------------------------------ saving

/// Save the settings and close the screen.
///
/// Deliberately not "write what `Settings` holds": [`Config::with_settings`]
/// starts from the config that was loaded and replaces only the keys the screen
/// can edit, so a key the screen knows nothing about is still in the file
/// afterwards. Today that is one key, and it is the one this screen's own line
/// editor writes -- `[spotify] client_id`. (A key `Config` itself does not model
/// cannot be kept this way, because the loader drops it before anything gets
/// here; that is 5.1's business, not this screen's.)
///
/// A save that fails leaves the screen up with the settings still in the app and
/// one line saying so, because closing on a failed write is how a day's
/// arranging is lost.
fn save_and_close(app: &mut App, path: &Path) {
    let next = app.config.with_settings(&app.settings);
    match next.save_to(path) {
        Ok(()) => {
            app.config = next;
            app.config_dirty = false;
            app.settings_open = false;
            forget_typing();
        }
        Err(e) => {
            app.toast = Some(Toast {
                text: e.notice(),
                at: Instant::now(),
            });
        }
    }
}

/// The file [`crate::config::Config::save`] writes. Spelled out rather than
/// calling `save`, so that a test can aim the very same save at a temporary
/// directory instead of at the owner's own config.
fn config_path() -> PathBuf {
    crate::config::Paths::from_env().file()
}

// ---------------------------------------------------------------- rendering

/// One line of the screen: a group heading, or a setting.
#[derive(Debug, Clone, Copy)]
enum Item {
    Group(&'static str),
    Row { index: usize, row: &'static Row },
}

/// The screen as a flat list of lines, with each setting's index in it.
///
/// Built rather than kept as a second table beside [`GROUPS`], because two lists
/// is how a setting ends up in the group but not in the list the cursor walks.
fn items() -> Vec<Item> {
    let mut out = Vec::new();
    let mut index = 0;
    for (name, rows) in GROUPS {
        out.push(Item::Group(name));
        for row in *rows {
            out.push(Item::Row { index, row });
            index += 1;
        }
    }
    out
}

/// Where the panel sits inside the area it is given.
///
/// Clamped to [`MAX_WIDTH`] and centred in what is left, so `trak config` on a
/// wide terminal gets a panel instead of a mostly-blank box. Never wider than
/// the area: the caller's rectangle is the hard limit, and it is how `,` sizes
/// the screen over the dashboard.
fn panel(area: Rect) -> Rect {
    let width = area.width.min(MAX_WIDTH);
    // With the key list beside it, the pair is centred rather than the panel.
    let x = if keys_panel(area).is_some() {
        area.x + (area.width - MAX_WIDTH - 1 - KEYS_WIDTH) / 2
    } else {
        area.x + area.width.saturating_sub(width) / 2
    };
    Rect { x, width, ..area }
}

/// Where the key list goes when it fits beside the settings, or `None`.
fn keys_panel(area: Rect) -> Option<Rect> {
    (area.width >= MAX_WIDTH + 1 + KEYS_WIDTH).then(|| Rect {
        x: area.x + (area.width - MAX_WIDTH - 1 - KEYS_WIDTH) / 2 + MAX_WIDTH + 1,
        width: KEYS_WIDTH,
        ..area
    })
}

/// How wide the key list's own panel is, when it fits beside the settings.
const KEYS_WIDTH: u16 = 56;

/// Every key trak has, one per line, from the table the dashboard's footer and
/// SPEC §4 are checked against.
fn key_lines(theme: &Theme) -> Vec<Line<'static>> {
    crate::tui::render::HELP_ROWS
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(
                    format!(" {k:<width$}", width = crate::tui::render::HELP_KEY_WIDTH),
                    theme.accent_style(),
                ),
                Span::raw(v.to_string()),
            ])
        })
        .collect()
}

/// The bottom row: a notice if there is one to read, the key hints otherwise.
///
/// The screen owns the keyboard, so its own notices are drawn here rather than
/// by the dashboard's toast -- `trak config` runs with no dashboard behind it,
/// and a save that failed has to say so in both.
fn footer_line(app: &App, theme: &Theme, width: usize) -> Paragraph<'static> {
    if let Some(toast) = &app.toast {
        return Paragraph::new(Line::from(Span::styled(
            format!(" {} ", toast.text),
            // Yellow rather than the accent: a notice about something not
            // having worked should not be drawn in the colour of the interface
            // working fine.
            Style::default().fg(Color::Yellow),
        )));
    }
    Paragraph::new(Line::from(Span::styled(
        hints(width, is_typing()),
        theme.accent_style(),
    )))
}

/// The keys, as many as fit the width. A hint cut off half way reads as a
/// different key, so a narrow panel loses the last ones rather than showing
/// them wrong.
fn hints(width: usize, typing: bool) -> String {
    let pairs: &[(&str, &str)] = if typing {
        &[("enter", "save"), ("backspace", "erase"), ("esc", "cancel")]
    } else {
        &[
            ("space", "toggle"),
            ("←/→", "change"),
            ("enter", "edit"),
            ("j/k", "move"),
            ("q", "save & close"),
        ]
    };
    let mut out = String::new();
    for (k, v) in pairs {
        let piece = format!("{k} {v}");
        // Two spaces between them: one reads as a typo in a list of keys.
        if !out.is_empty() && out.chars().count() + piece.chars().count() + 2 > width {
            break;
        }
        if !out.is_empty() {
            out.push_str("  ");
        }
        out.push_str(&piece);
    }
    if out.is_empty() {
        return out;
    }
    out.insert(0, ' ');
    out
}

/// One line: a heading, or a setting with its value.
fn line_for(item: &Item, app: &App, theme: &Theme) -> Line<'static> {
    match item {
        Item::Group(name) => {
            let mut spans = vec![Span::styled(
                *name,
                theme.accent_style().add_modifier(Modifier::BOLD),
            )];
            // The guided setup (TODO 7.3) has no row of its own, and the hints
            // row has no room for another key, so the heading it belongs to says
            // how to reach it.
            if *name == SPOTIFY_GROUP {
                spans.push(Span::styled(HINT_SETUP, Theme::dim()));
            }
            Line::from(spans)
        }
        Item::Row { index, row } => {
            let (label, _) = *row;
            // The value being typed takes the row's place, so the thing being
            // edited is where the eye already is.
            let value = if *index == cursor_of(app) && is_typing() {
                typed().unwrap_or_default()
            } else {
                value_of(row, app)
            };
            let line = Line::from(vec![
                Span::raw(format!("{:<LABEL_WIDTH$}", label)),
                Span::raw(" "),
                Span::raw(value),
            ]);
            // The cursor is the whole row inverted rather than a caret in a
            // column of its own: a marker's width is ambiguous in some
            // terminals, and a value column that shifted as the cursor moved
            // would be worse than no marker at all.
            if *index == cursor_of(app) {
                line.patch_style(Style::default().add_modifier(Modifier::REVERSED))
            } else {
                line
            }
        }
    }
}

/// The value a row shows, in the words the file writes it under.
fn value_of(row: &Row, app: &App) -> String {
    let (_, control) = *row;
    match control {
        Control::Toggle(which) => if which.get(app) { "on" } else { "off" }.to_string(),
        Control::Choice(which) => which.text(app).to_string(),
        Control::Number(which) => which.text(app),
        Control::Text { get, .. } => {
            let value = get(app);
            // An empty Client ID is Version B rather than a missing value, and
            // saying so is the difference between "trak is broken" and "trak is
            // working with the Free tier".
            if value.is_empty() {
                "not set".to_string()
            } else {
                value
            }
        }
    }
}

/// Where the terminal's own cursor goes: just past the last character typed, on
/// the line the edited row is drawn on.
///
/// The hardware cursor rather than a character in the text, so the line cannot
/// end with a box-drawing glyph of ambiguous width. `None` when nothing is being
/// typed, and ratatui hides the cursor for a frame that does not set it, so
/// closing the line editor does not leave a caret stranded somewhere.
fn caret(app: &App, rows: Rect, scroll: usize) -> Option<Position> {
    if !is_typing() || rows.height == 0 {
        return None;
    }
    let line = typed()?;
    let column = rows.x + LABEL_WIDTH as u16 + 1;
    // Never outside the panel: a long Client ID would otherwise put the caret in
    // the next pane.
    let x = (column + line.chars().count() as u16).min(rows.x + rows.width.saturating_sub(1));
    let y = rows.y + line_of(cursor_of(app)).saturating_sub(scroll) as u16;
    (y < rows.y + rows.height).then_some(Position { x, y })
}

// ------------------------------------------------------------------ helpers

/// The next value `way` away in `list`, wrapping at both ends, so `←` on the
/// first choice lands on the last rather than on nothing.
///
/// The lists are the enums' own `ALL`, so a value off the list cannot happen;
/// `unwrap_or(0)` is only here so the function has no way to panic.
fn cycled<T: Copy + PartialEq>(list: &[T], current: T, way: Way) -> T {
    if list.is_empty() {
        return current;
    }
    let i = list.iter().position(|v| *v == current).unwrap_or(0);
    match way {
        Way::Right => list[(i + 1) % list.len()],
        Way::Left => list[(i + list.len() - 1) % list.len()],
    }
}

/// The next value `way` along a sorted ladder.
///
/// A value that is not on the ladder -- hand-edited, or left by a newer trak --
/// moves to the nearest entry on the side that was asked for, and if there is
/// none, to the end. That is what makes the ladder the range rather than a
/// suggestion: a step size can be out of range on the way in, and one press in
/// either direction brings it onto the ladder for good.
fn along<T: Copy + PartialOrd>(ladder: &[T], current: T, way: Way) -> T {
    let found = match way {
        Way::Right => ladder
            .iter()
            .find(|v| **v > current)
            .or_else(|| ladder.last()),
        Way::Left => ladder
            .iter()
            .rev()
            .find(|v| **v < current)
            .or_else(|| ladder.first()),
    };
    found.copied().unwrap_or(current)
}

// ------------------------------------------------------- the typed line

fn with_typing<T>(f: impl FnOnce(&mut Option<String>) -> T) -> T {
    TYPING.with(|line| f(&mut line.borrow_mut()))
}

fn is_typing() -> bool {
    with_typing(|line| line.is_some())
}

fn typed() -> Option<String> {
    with_typing(|line| line.clone())
}

fn set_typing(line: Option<String>) {
    with_typing(|slot| *slot = line);
}

fn take_typing() -> Option<String> {
    with_typing(|line| line.take())
}

fn forget_typing() {
    set_typing(None);
}

fn map_typing(f: impl FnOnce(&mut String)) {
    with_typing(|line| {
        if let Some(line) = line {
            f(line);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use ratatui::Terminal;
    use ratatui::backend::{Backend, TestBackend};
    use ratatui::buffer::Buffer;

    use crate::config::{Config, tab_from_config};

    /// A temporary directory that removes itself, as `config.rs`'s tests do: no
    /// dependency for one struct, and the name carries the test so two tests in
    /// one process cannot share it.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "trak-settings-test-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An open screen over a fresh app. Every test starts here rather than with a
    /// bare `App` because `open` is what clears the line being typed, and that
    /// state is per thread: a test that skipped it could inherit a half-typed
    /// Client ID from whichever test ran before it on the same thread.
    fn opened() -> App {
        let mut app = App::new();
        open(&mut app);
        app
    }

    /// An open screen with a row of the table under the cursor.
    fn opened_at(index: usize) -> App {
        let mut app = opened();
        app.settings_cursor = index;
        app
    }

    /// The index of a setting in screen order, by the key the file writes it
    /// under. Going through the label rather than a hand-counted number is what
    /// makes a row inserted above another one a non-event.
    fn index_of(label: &str) -> usize {
        GROUPS
            .iter()
            .flat_map(|(_, rows)| rows.iter())
            .position(|(key, _)| *key == label)
            .expect("a setting with that key")
    }

    /// Draw the screen over the whole frame, which is what `trak config` does.
    fn drawn(w: u16, h: u16, app: &App) -> String {
        to_text(&buffer_of(w, h, app))
    }

    fn buffer_of(w: u16, h: u16, app: &App) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).expect("a test terminal");
        term.draw(|f| render(f, f.area(), app, &Theme::default()))
            .expect("drawing the settings screen");
        term.backend().buffer().clone()
    }

    /// One buffer as text, a line per row.
    fn to_text(buf: &Buffer) -> String {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The panel the screen draws in a frame of `w` x `h`, which is where the
    /// value column starts.
    fn panel_at(w: u16, h: u16) -> Rect {
        panel(Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        })
    }

    /// The cell the values start in.
    fn value_column(w: u16, h: u16) -> u16 {
        panel_at(w, h).x + 1 + LABEL_WIDTH as u16 + 1
    }

    /// The value drawn for one setting, read back out of the frame.
    ///
    /// Read from the cells rather than from the table, so a test fails when the
    /// screen stops *showing* a setting and not only when it stops computing it.
    /// The row is found by the label in the label column rather than by a
    /// substring, so `art` never finds `art_protocol`.
    fn drawn_value(w: u16, h: u16, app: &App, label: &str) -> String {
        let buf = buffer_of(w, h, app);
        let panel = panel_at(w, h);
        let left = panel.x + 1;
        let column = value_column(w, h);
        for y in 0..buf.area.height {
            let key: String = (left..buf.area.right())
                .take(LABEL_WIDTH)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string();
            if key == label {
                return (column..panel.right() - 1)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string();
            }
        }
        panic!("{label} is not on the screen:\n{}", to_text(&buf))
    }

    /// `trak config` runs this screen with no Spotify, no track and no event
    /// loop, so it has to draw against a bare app (SPEC §9, TODO 5.2).
    #[test]
    fn it_draws_against_a_bare_app() {
        let text = drawn(80, 30, &opened());
        for group in [
            "Display",
            "Theme",
            "Visualizer",
            "Input",
            "Notifications",
            "Spotify API",
        ] {
            assert!(text.contains(group), "{group} is missing from:\n{text}");
        }
        assert!(text.contains("art"), "{text}");
        assert!(text.contains("client_id"), "{text}");
    }

    /// The whole screen, so a change to the layout or to a value shows up in the
    /// diff rather than in somebody's terminal.
    ///
    /// Drawn at [`MAX_WIDTH`], where the panel is the whole frame, so there is no
    /// padding to go stale in the expected text.
    #[test]
    fn the_screen_at_sixty_eight_by_thirty() {
        let text = drawn(MAX_WIDTH, 30, &opened());
        let expected = concat!(
            "╭ Settings ──────────────────────────────────── changes apply live ╮\n",
            "│Display                                                           │\n",
            "│art              on                                               │\n",
            "│mode             art                                              │\n",
            "│progress         on                                               │\n",
            "│volume           on                                               │\n",
            "│popularity       on                                               │\n",
            "│key_hints        on                                               │\n",
            "│clock            on                                               │\n",
            "│side_pane        on                                               │\n",
            "│default_tab      history                                          │\n",
            "│enabled          on                                               │\n",
            "│Theme                                                             │\n",
            "│border           rounded                                          │\n",
            "│accent           art                                              │\n",
            "│art_protocol     auto                                             │\n",
            "│Visualizer                                                        │\n",
            "│style            spectrum                                         │\n",
            "│source           auto                                             │\n",
            "│Input                                                             │\n",
            "│mouse            on                                               │\n",
            "│volume_step      10                                               │\n",
            "│control          spotify                                          │\n",
            "│Notifications                                                     │\n",
            "│song_change      off                                              │\n",
            "│Spotify API  ·  s: guided setup                                   │\n",
            "│client_id        not set                                          │\n",
            // The filler row that keeps the hint line on the bottom edge. It is
            // here because the table lost a row with the seek step and the screen
            // still has to fill its height: a hint line floating half way up
            // looks like a different screen.
            "│                                                                  │\n",
            "│ space toggle  ←/→ change  enter edit  j/k move  q save & close   │\n",
            "╰──────────────────────────────────────────────────────────────────╯",
        );
        assert_eq!(text, expected);
    }

    /// The same screen in a terminal that cannot show all of it: the view has to
    /// scroll and keep the cursor on screen, with no torn border.
    #[test]
    fn a_narrow_terminal_scrolls_and_keeps_its_border_whole() {
        let mut app = opened();
        // Down to the bottom of the table.
        for _ in 0..row_count() {
            key(&mut app, 'j');
        }
        let text = drawn(46, 16, &app);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 16, "{text}");
        for (i, line) in lines.iter().enumerate() {
            assert_eq!(line.chars().count(), 46, "row {i} is torn: {line:?}");
        }
        // The last setting, and nothing above it, is what a full-height pane at
        // the bottom of the table shows.
        assert!(text.contains("client_id"), "{text}");
        assert!(!text.contains("Notifications\n"), "{text}");
    }

    /// Below the size the checklist can be read at, SPEC §3 asks for a word about
    /// the terminal rather than something torn.
    #[test]
    fn a_tiny_terminal_is_told_to_resize() {
        for (w, h) in [(20u16, 6u16), (29, 7), (29, 8), (30, 7), (12, 4)] {
            let text = drawn(w, h, &opened());
            let words: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
            assert_eq!(words, "terminal too small — resize", "at {w}x{h}");
            // Nothing is drawn but the note, in particular no border to tear.
            assert!(
                !text.contains(['╭', '╰', '│']),
                "something was drawn at {w}x{h}:\n{text}"
            );
        }
        // Too small for the note to be legible, and still no border.
        let text = drawn(4, 2, &opened());
        assert!(!text.contains(['╭', '╰', '│']), "{text}");
    }

    /// SPEC §3: the layout is computed from the terminal size on every frame, so
    /// every size has to be survivable rather than just the tested ones.
    #[test]
    fn it_never_panics_at_any_size() {
        for w in [20u16, 30, 45, 64, 65, 100, 200] {
            for h in [4u16, 7, 8, 12, 16, 30, 60] {
                for cursor in [0, 7, row_count() - 1, 999] {
                    let mut app = opened();
                    app.settings_cursor = cursor;
                    let _ = drawn(w, h, &app);
                }
            }
        }
    }

    /// Every key SPEC §8 defines has a row. Read out of the document rather than
    /// copied into this test, so the check cannot pass because a list here fell
    /// behind -- which is the whole of "every key in SPEC §8 is editable".
    #[test]
    fn every_key_the_spec_lists_has_a_row() {
        let labels: Vec<&str> = row_labels();
        for key in spec_keys() {
            assert!(
                labels.contains(&key.as_str()),
                "SPEC §8's {key:?} has no row"
            );
        }
    }

    /// The other half of the same check: a row is a key the file actually
    /// writes, so the screen cannot offer something that is silently dropped on
    /// the next save.
    #[test]
    fn every_row_is_a_key_the_file_writes() {
        let toml = Config::default().to_toml();
        let mut seen: Vec<&str> = Vec::new();
        for (_, rows) in GROUPS {
            for (label, _) in *rows {
                let label = *label;
                let line = format!("{label} = ");
                assert!(toml.contains(&line), "{label} is not a key in the file");
                assert!(!seen.contains(&label), "{label} is listed twice");
                seen.push(label);
            }
        }
    }

    /// The six groups, in the order the task names them, with the settings that
    /// belong to each. Pinned because a group boundary is a product decision
    /// (`[display] border` reading as "Theme") rather than a detail.
    #[test]
    fn the_groups_are_the_six_the_spec_lists() {
        let got: Vec<&str> = GROUPS.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            got,
            [
                "Display",
                "Theme",
                "Visualizer",
                "Input",
                "Notifications",
                "Spotify API"
            ]
        );
        let labels = |group: &str| -> Vec<&str> {
            GROUPS
                .iter()
                .find(|(name, _)| *name == group)
                .map(|(_, rows)| rows.iter().map(|(key, _)| *key).collect())
                .unwrap_or_default()
        };
        assert_eq!(
            labels("Display"),
            [
                "art",
                "mode",
                "progress",
                "volume",
                "popularity",
                "key_hints",
                "clock",
                "side_pane",
                "default_tab",
                "enabled"
            ]
        );
        assert_eq!(labels("Theme"), ["border", "accent", "art_protocol"]);
        assert_eq!(labels("Visualizer"), ["style", "source"]);
        assert_eq!(labels("Input"), ["mouse", "volume_step", "control"]);
        assert_eq!(labels("Notifications"), ["song_change"]);
        assert_eq!(labels("Spotify API"), ["client_id"]);
    }

    /// Every on/off setting, flipped the way a person flips it, with the change
    /// read back off the screen. This is TODO 5.3's "a test per setting flips it
    /// and asserts the render changes" for the settings where that is true.
    #[test]
    fn space_flips_every_toggle_and_the_screen_shows_it() {
        for (label, was, now) in TOGGLES {
            let (was, now) = (*was, *now);
            let mut app = opened_at(index_of(label));
            assert_eq!(
                drawn_value(64, 30, &app, label),
                was,
                "{label} starts wrong"
            );
            key(&mut app, ' ');
            assert_eq!(
                drawn_value(64, 30, &app, label),
                now,
                "{label} did not flip"
            );
            key(&mut app, ' ');
            assert_eq!(
                drawn_value(64, 30, &app, label),
                was,
                "{label} did not flip back"
            );
            assert!(app.config_dirty, "{label} left the config marked clean");
        }
    }

    /// `←`/`→` walk every choice, in both directions, and the screen says so.
    #[test]
    fn the_arrows_change_every_choice() {
        for (label, from, to) in CHOICES {
            let (from, to) = (*from, *to);
            let mut app = opened_at(index_of(label));
            assert_eq!(
                drawn_value(64, 30, &app, label),
                from,
                "{label} starts wrong"
            );
            key(&mut app, 'l');
            assert_eq!(
                drawn_value(64, 30, &app, label),
                to,
                "{label} did not step right"
            );
            key(&mut app, 'l');
            assert_ne!(
                drawn_value(64, 30, &app, label),
                to,
                "{label} stuck on {to}"
            );
            key(&mut app, 'h');
            assert_eq!(
                drawn_value(64, 30, &app, label),
                to,
                "{label} did not step back"
            );
        }
    }

    /// The two numbers step along their ladders rather than accepting a typed
    /// value, and the screen shows the number.
    #[test]
    fn the_arrows_walk_the_step_ladders() {
        let mut app = opened_at(index_of("volume_step"));
        assert_eq!(drawn_value(64, 30, &app, "volume_step"), "10");
        key(&mut app, 'l');
        assert_eq!(drawn_value(64, 30, &app, "volume_step"), "15");
        key(&mut app, 'h');
        key(&mut app, 'h');
        assert_eq!(drawn_value(64, 30, &app, "volume_step"), "5");
    }

    /// A number is walked along a ladder, so no number of presses can produce a
    /// step outside the range the rest of trak can use -- including from a value
    /// a hand-edited file put there, which is the only way one could arrive.
    #[test]
    fn a_step_size_cannot_leave_its_range() {
        for start in [-300i16, 0, 1, 7, 50, 100, 900] {
            let mut app = opened_at(index_of("volume_step"));
            app.settings.volume_step = start;
            for _ in 0..12 {
                key(&mut app, 'h');
                assert!(
                    (1..=100).contains(&app.settings.volume_step),
                    "left from {start} gave {}",
                    app.settings.volume_step
                );
            }
            for _ in 0..12 {
                key(&mut app, 'l');
                assert!(
                    (1..=100).contains(&app.settings.volume_step),
                    "right from {start} gave {}",
                    app.settings.volume_step
                );
            }
        }
    }

    /// A ladder is walked, not clamped to a fixed pair of ends: `1` is as much of
    /// a volume step as `50`, and a person who wants it can have it.
    #[test]
    fn a_ladder_stops_at_its_ends() {
        assert_eq!(along(&VOLUME_STEPS, 1, Way::Left), 1);
        assert_eq!(along(&VOLUME_STEPS, 50, Way::Right), 50);
        // Off the ladder: the nearest entry on the side that was asked for.
        assert_eq!(along(&VOLUME_STEPS, 7, Way::Right), 10);
        assert_eq!(along(&VOLUME_STEPS, 7, Way::Left), 5);
        // Below the ladder, either way onto it: a file that says 0 gets fixed by
        // the first press rather than staying broken.
        assert_eq!(along(&VOLUME_STEPS, 0, Way::Right), 1);
        assert_eq!(along(&VOLUME_STEPS, 900, Way::Right), 50);
        assert_eq!(along(&VOLUME_STEPS, -300, Way::Left), 1);
    }

    /// The cursor walks the settings and stops at both ends, and a cursor that
    /// points nowhere -- a plain `App` from before the field existed, say --
    /// cannot index off the end of anything.
    #[test]
    fn the_cursor_walks_the_settings_and_stops() {
        let mut app = opened();
        assert_eq!(app.settings_cursor, 0);
        for _ in 0..40 {
            key(&mut app, 'j');
        }
        assert_eq!(app.settings_cursor, row_count() - 1);
        assert_eq!(drawn_value(64, 30, &app, "client_id"), "not set");
        for _ in 0..40 {
            key(&mut app, 'k');
        }
        assert_eq!(app.settings_cursor, 0);
        assert_eq!(drawn_value(64, 30, &app, "art"), "on");

        app.settings_cursor = usize::MAX;
        key(&mut app, 'j');
        assert_eq!(app.settings_cursor, row_count() - 1);
        let _ = drawn(64, 30, &app);
    }

    /// The cursor is drawn: the row under it is inverted, and the rows either
    /// side of it are not, so it cannot be lost in a long list.
    #[test]
    fn the_cursor_is_visible_on_the_row_it_is_on() {
        for label in ["art", "accent", "client_id"] {
            let app = opened_at(index_of(label));
            let buf = buffer_of(64, 30, &app);
            let x = value_column(64, 30) - 1;
            let inverted: Vec<u16> = (0..buf.area.height)
                .filter(|y| {
                    buf[(x, *y)]
                        .style()
                        .add_modifier
                        .contains(Modifier::REVERSED)
                })
                .collect();
            let text = to_text(&buf);
            assert_eq!(
                inverted.len(),
                1,
                "not one inverted row for {label}:\n{text}"
            );
            let on = inverted[0];
            assert!(
                text.lines()
                    .nth(on as usize)
                    .unwrap_or_default()
                    .contains(label),
                "the inverted row is not {label}:\n{text}"
            );
        }
    }

    /// Whatever size and wherever the cursor, the row it is on is on the screen.
    /// A cursor that scrolls off is the one failure mode of a scrolled list that
    /// no other test here would catch.
    #[test]
    fn the_cursor_is_always_drawn() {
        for w in [30u16, 46, 64, 120] {
            for h in [8u16, 12, 16, 40] {
                for cursor in 0..row_count() {
                    let app = opened_at(cursor);
                    let text = drawn(w, h, &app);
                    let label = row_by_index(cursor).map_or("", |(key, _)| *key);
                    assert!(
                        text.contains(label),
                        "{label} at {w}x{h} is off screen:\n{text}"
                    );
                }
            }
        }
    }

    /// `q` saves and closes, and what it wrote reads back as what was on screen.
    #[test]
    fn q_saves_and_closes() {
        let dir = TempDir::new("save");
        let path = dir.join("config.toml");
        let mut app = opened_at(index_of("border"));
        app.settings.border = crate::tui::theme::Border::Double;
        save_and_close(&mut app, &path);

        assert!(!app.settings_open, "the screen is still up");
        assert!(!app.config_dirty, "the config is still marked unsaved");
        let body = std::fs::read_to_string(&path).expect("the config was written");
        assert!(body.contains("border = \"double\""), "{body}");
        let loaded = Config::load_from(&path).expect("load");
        assert_eq!(
            loaded.config.display.border,
            crate::tui::theme::Border::Double
        );
    }

    /// `esc` is the same key, because a `,`-opened screen has to close the way
    /// every other overlay in trak does.
    #[test]
    fn esc_saves_and_closes() {
        let dir = TempDir::new("esc");
        let path = dir.join("config.toml");
        let mut app = opened_at(index_of("clock"));
        key(&mut app, ' ');
        assert!(!app.settings.show_clock, "space did not turn the clock off");
        save_and_close(&mut app, &path);
        assert!(!app.settings_open);
        assert!(
            std::fs::read_to_string(&path)
                .expect("read")
                .contains("clock = false")
        );
    }

    /// The reason the save goes through `with_settings` rather than being
    /// written from `Settings`: every key the config models that the screen does
    /// not edit is still in the file afterwards. Today that is one key, and it
    /// is the one the screen's own line editor writes -- `[spotify] client_id`.
    /// A save that rebuilt the file from `Settings` would delete it.
    #[test]
    fn a_key_the_screen_does_not_edit_survives_the_save() {
        let dir = TempDir::new("client");
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[display]\nborder = \"sharp\"\n\n[spotify]\nclient_id = \"a-client-id\"\n",
        )
        .expect("write");
        let loaded = Config::load_from(&path).expect("load");
        let mut app = App::new();
        app.config = loaded.config;
        app.settings = app.config.settings();
        open(&mut app);

        app.settings_cursor = index_of("key_hints");
        key(&mut app, ' ');
        save_and_close(&mut app, &path);

        let body = std::fs::read_to_string(&path).expect("read");
        assert!(
            body.contains("client_id = \"a-client-id\""),
            "a saved Client ID was deleted:\n{body}"
        );
        assert!(
            body.contains("key_hints = false"),
            "the change did not take:\n{body}"
        );
        // The screen shows what the file said, which is the other half of it.
        assert_eq!(drawn_value(64, 30, &app, "client_id"), "a-client-id");
    }

    /// A save that fails must not close the screen, must not lose what was
    /// changed, and must say so in one line.
    #[test]
    fn a_failed_save_keeps_the_screen_the_settings_and_says_so() {
        let dir = TempDir::new("failed");
        // A file where the config directory should be: the write cannot succeed,
        // and it fails the way a disk problem does rather than by panicking.
        std::fs::write(dir.join("blocked"), "not a directory").expect("write");
        let path = dir.join("blocked").join("config.toml");

        let mut app = opened_at(index_of("border"));
        app.settings.border = crate::tui::theme::Border::Sharp;
        save_and_close(&mut app, &path);

        assert!(app.settings_open, "a failed save closed the screen");
        assert_eq!(
            app.settings.border,
            crate::tui::theme::Border::Sharp,
            "the change was lost"
        );
        let toast = app.toast.as_ref().expect("no notice for a failed save");
        assert!(toast.text.contains("could not"), "{}", toast.text);
        assert_eq!(toast.text.lines().count(), 1, "the notice is not one line");
        // One line, and the panel clips the tail of a long path rather than
        // wrapping the notice over the checklist.
        let shown = toast.text.chars().take(30).collect::<String>();
        assert!(
            drawn(64, 30, &app).contains(&shown),
            "the notice is not on screen"
        );
    }

    /// The screen's own notices are drawn on its bottom row, because
    /// `trak config` has no dashboard behind it to draw a toast over.
    #[test]
    fn the_notice_takes_the_bottom_row() {
        let app = opened();
        assert!(drawn(MAX_WIDTH, 30, &app).contains("q save & close"));
        let mut app = app;
        app.toast = Some(Toast {
            text: "trak: could not write x".into(),
            at: Instant::now(),
        });
        let text = drawn(MAX_WIDTH, 30, &app);
        assert!(text.contains("trak: could not write x"), "{text}");
        assert!(!text.contains("q save & close"), "{text}");
    }

    /// `enter` on the Client ID opens a line, seeded with what is there, and the
    /// terminal's own cursor is put after the last character of it.
    #[test]
    fn enter_opens_the_line_editor_on_the_client_id() {
        let mut app = opened_at(index_of("client_id"));
        app.config.spotify.client_id = "abc123".into();
        key(&mut app, '\n');
        assert!(is_typing());
        assert!(drawn(64, 30, &app).contains("abc123"));

        let mut term = Terminal::new(TestBackend::new(64, 30)).expect("a test terminal");
        term.draw(|f| render(f, f.area(), &app, &Theme::default()))
            .expect("draw");
        let at = term
            .backend_mut()
            .get_cursor_position()
            .expect("cursor position");
        assert_eq!(
            at.x,
            value_column(64, 30) + 6,
            "the caret is not after the text"
        );
    }

    /// What is typed is what is committed, trimmed, and the screen goes back to
    /// the checklist rather than closing.
    #[test]
    fn the_line_editor_commits_on_enter() {
        let mut app = opened_at(index_of("client_id"));
        key(&mut app, '\n');
        for c in "0123456789abcdef0123456789abcdef".chars() {
            key(&mut app, c);
        }
        key(&mut app, '\n');
        assert!(!is_typing());
        assert!(app.settings_open, "committing closed the screen");
        assert_eq!(
            app.config.spotify.client_id,
            "0123456789abcdef0123456789abcdef"
        );
        assert!(app.config_dirty);
        assert_eq!(
            drawn_value(64, 30, &app, "client_id"),
            "0123456789abcdef0123456789abcdef"
        );
    }

    /// Backspace erases and `esc` throws the line away, which is the difference
    /// between a text field and a commit you cannot take back.
    #[test]
    fn the_line_editor_erases_and_cancels() {
        let mut app = opened_at(index_of("client_id"));
        app.config.spotify.client_id = "keep".into();
        key(&mut app, '\n');
        // Seeded with what was already in the setting, so an edit starts from
        // the value rather than from nothing.
        for c in "junk".chars() {
            key(&mut app, c);
        }
        assert_eq!(typed().as_deref(), Some("keepjunk"));
        key(&mut app, '\x08');
        assert_eq!(typed().as_deref(), Some("keepjun"));
        key(&mut app, '\x1b');
        assert!(!is_typing());
        assert_eq!(
            app.config.spotify.client_id, "keep",
            "a cancelled edit was committed"
        );
        assert_eq!(drawn_value(64, 30, &app, "client_id"), "keep");
    }

    /// A control character is a key the terminal wanted, not one a person typed
    /// into a credential.
    #[test]
    fn the_line_editor_drops_control_characters() {
        let mut app = opened_at(index_of("client_id"));
        key(&mut app, '\n');
        for c in ['a', '\u{3}', '\t', 'b'] {
            key(&mut app, c);
        }
        assert_eq!(typed().as_deref(), Some("ab"));
    }

    /// A Client ID longer than the value column is still fully committed: the
    /// screen is allowed to clip it, the file is not.
    #[test]
    fn a_clipped_client_id_is_still_written_out_in_full() {
        let id = "0123456789abcdef0123456789abcdef0123456789";
        let mut app = opened_at(index_of("client_id"));
        key(&mut app, '\n');
        for c in id.chars() {
            key(&mut app, c);
        }
        key(&mut app, '\n');
        assert_eq!(app.config.spotify.client_id, id);
        // And the panel does not tear around it.
        let text = drawn(40, 12, &app);
        for line in text.lines() {
            assert_eq!(line.chars().count(), 40, "a row is torn: {line:?}");
        }
    }

    /// A typed Client ID reaches the file, because the commit is on the config
    /// and the save starts from the config.
    #[test]
    fn a_typed_client_id_is_saved() {
        let dir = TempDir::new("client");
        let path = dir.join("config.toml");
        let mut app = opened_at(index_of("client_id"));
        key(&mut app, '\n');
        for c in "deadbeef".chars() {
            key(&mut app, c);
        }
        key(&mut app, '\n');
        save_and_close(&mut app, &path);
        assert!(
            std::fs::read_to_string(&path)
                .expect("read")
                .contains("client_id = \"deadbeef\"")
        );
    }

    /// `,` starts over: a fresh cursor, and no half-typed line carried in from
    /// a screen that was left open.
    #[test]
    fn open_starts_again() {
        let mut app = opened_at(index_of("client_id"));
        key(&mut app, '\n');
        key(&mut app, 'x');
        assert!(is_typing());
        open(&mut app);
        assert_eq!(app.settings_cursor, 0);
        assert!(!is_typing(), "a half-typed line survived a reopen");
        assert_eq!(
            app.config.spotify.client_id, "",
            "typing changed the setting by itself"
        );
    }

    /// A setting whose whole range is a choice, a number and a line of text: the
    /// words on screen are the words the file reads. A screen that shows a value
    /// the parser would refuse is worse than no screen.
    #[test]
    fn the_words_on_screen_are_the_words_the_file_reads() {
        for accent in ACCENTS {
            assert_eq!(Accent::parse(accent_name(accent)), Some(accent));
        }
        for mode in MODES {
            assert_eq!(DisplayMode::parse(mode_name(mode)), Some(mode));
        }
        for tab in Tab::ALL {
            assert_eq!(tab_from_config(tab_name(tab)), Some(tab));
        }
        for control in VOLUME_CONTROLS {
            assert_eq!(VolumeControl::parse(control.label()), Some(control));
        }
        for protocol in ArtProtocol::ALL {
            assert_eq!(ArtProtocol::parse(protocol.label()), Some(protocol));
        }
        for source in VisualizerSource::ALL {
            assert_eq!(VisualizerSource::parse(source.label()), Some(source));
        }
        for border in Border::ALL {
            assert_eq!(Border::parse(border.name()), Some(border));
        }
        for style in VisualizerStyle::ALL {
            assert_eq!(VisualizerStyle::parse(style.label()), Some(style));
        }
    }

    /// The numbers the screen writes are the numbers the file writes, so the
    /// file does not say `5.0` about a setting the screen called `5`.
    #[test]
    fn a_step_size_reads_back_as_the_screen_showed_it() {
        let dir = TempDir::new("steps");
        let path = dir.join("config.toml");
        let mut app = opened_at(index_of("volume_step"));
        key(&mut app, 'h');
        let shown = drawn_value(64, 30, &app, "volume_step");
        save_and_close(&mut app, &path);
        let body = std::fs::read_to_string(&path).expect("read");
        assert!(
            body.contains(&format!("volume_step = {shown}")),
            "the screen said {shown}:\n{body}"
        );
    }

    /// `space` and `←`/`→` do nothing on the rows they do not apply to, rather
    /// than guessing at what the user might have meant.
    #[test]
    fn a_key_does_nothing_on_a_row_it_does_not_apply_to() {
        for (label, key_char) in [
            ("client_id", ' '),
            ("client_id", 'l'),
            ("art", 'l'),
            ("volume_step", ' '),
            ("volume_step", '\n'),
            ("border", ' '),
            ("art", '\n'),
        ] {
            let mut app = opened_at(index_of(label));
            let before = drawn_value(64, 30, &app, label);
            key(&mut app, key_char);
            assert_eq!(
                drawn_value(64, 30, &app, label),
                before,
                "{key_char:?} changed {label}"
            );
            assert!(app.settings_open, "{key_char:?} closed the screen");
        }
    }

    /// Every value is in the same column, and every label ends before it. A
    /// caret or a marker in a column of its own would be ambiguous-width in some
    /// terminals, and the whole value column shifting as the cursor moved is
    /// worse than having no marker at all.
    #[test]
    fn every_value_is_in_the_same_column() {
        let app = opened();
        let buf = buffer_of(64, 30, &app);
        let panel = panel_at(64, 30);
        let column = value_column(64, 30);
        for label in row_labels() {
            let y = row_of(&buf, label).unwrap_or_else(|| panic!("{label} is not on screen"));
            assert_eq!(
                buf[(column - 1, y)].symbol(),
                " ",
                "there is no gap between {label} and its value"
            );
            assert_ne!(
                buf[(column, y)].symbol(),
                " ",
                "{label} has no value in the value column"
            );
            let cells = (column..panel.right() - 1)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>();
            assert_eq!(
                cells.trim_end(),
                drawn_value(64, 30, &app, label),
                "{label} reads oddly"
            );
        }
    }

    /// Every key the screen offers, in screen order.
    fn row_labels() -> Vec<&'static str> {
        GROUPS
            .iter()
            .flat_map(|(_, rows)| rows.iter())
            .map(|(key, _)| *key)
            .collect()
    }

    /// The row a setting is drawn on, found by its label.
    fn row_of(buf: &Buffer, label: &str) -> Option<u16> {
        let left = panel_at(buf.area.width, buf.area.height).x + 1;
        (0..buf.area.height).find(|y| {
            (left..buf.area.right())
                .take(LABEL_WIDTH)
                .map(|x| buf[(x, *y)].symbol())
                .collect::<String>()
                .trim_end()
                == label
        })
    }

    /// The hints say what the keys do, and say the other thing while a line is
    /// being typed, because that is when they are different.
    #[test]
    fn the_hints_follow_what_the_keys_do() {
        let app = opened();
        let text = drawn(MAX_WIDTH, 30, &app);
        for hint in [
            "space toggle",
            "←/→ change",
            "enter edit",
            "j/k move",
            "q save & close",
        ] {
            assert!(text.contains(hint), "{hint} is missing from:\n{text}");
        }
        // All of them at the widest the panel is ever drawn, so narrowing it
        // cannot quietly drop the one that matters.
        assert!(
            hints(MAX_WIDTH as usize - 2, false).ends_with("q save & close"),
            "a hint does not fit at MAX_WIDTH"
        );
        let mut app = app;
        app.settings_cursor = index_of("client_id");
        key(&mut app, '\n');
        let text = drawn(64, 30, &app);
        assert!(text.contains("esc cancels"), "{text}");
        assert!(text.contains("backspace erase"), "{text}");
        assert!(!text.contains("space toggle"), "{text}");
    }

    /// The panel is narrower than a wide terminal and centred in it, and never
    /// wider than the rectangle it was given.
    #[test]
    fn the_panel_fits_the_space_it_was_given() {
        for w in [30u16, 64, 65, 200] {
            let app = opened();
            let buf = buffer_of(w, 30, &app);
            let panel = panel_at(w, 30);
            assert_eq!(
                panel.width,
                w.min(MAX_WIDTH),
                "the panel is the wrong width at {w}"
            );
            for y in 0..buf.area.height {
                // Every row is the width of the frame: nothing is torn.
                assert_eq!(
                    (0..buf.area.width).count(),
                    w as usize,
                    "row {y} is torn at {w}"
                );
                assert!(
                    (0..buf.area.width).all(|x| buf[(x, y)].symbol().chars().count() == 1),
                    "row {y} holds a wide character at {w}, so it is torn"
                );
                // The border is where the panel is, and blank everywhere else.
                assert_eq!(
                    buf[(panel.x, y)].symbol(),
                    edge(y, buf.area.height, false),
                    "at {w}x{y}"
                );
                assert_eq!(
                    buf[(panel.right() - 1, y)].symbol(),
                    edge(y, buf.area.height, true),
                    "at {w}x{y}"
                );
                let keys = keys_panel(buf.area);
                for x in (0..panel.x)
                    .chain(panel.right()..buf.area.width)
                    .filter(|x| keys.is_none_or(|k| !(k.x..k.right()).contains(x)))
                {
                    assert_eq!(
                        buf[(x, y)].symbol(),
                        " ",
                        "drawn outside the panel at ({x}, {y})"
                    );
                }
            }
        }
    }

    /// `,` over the dashboard hands `render` a rectangle, and the screen uses it
    /// rather than the whole frame. The dashboard behind it is not disturbed
    /// outside the panel.
    #[test]
    fn the_overlay_stays_inside_the_rectangle_it_is_given() {
        let app = opened();
        let area = Rect {
            x: 4,
            y: 2,
            width: 30,
            height: 10,
        };
        let mut term = Terminal::new(TestBackend::new(50, 20)).expect("a test terminal");
        term.draw(|f| render(f, area, &app, &Theme::default()))
            .expect("draw");
        let buf = term.backend().buffer();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let inside = x >= area.x
                    && x < area.x + area.width
                    && y >= area.y
                    && y < area.y + area.height;
                if !inside {
                    assert_eq!(
                        buf[(x, y)].symbol(),
                        " ",
                        "cell ({x}, {y}) outside the overlay was drawn on"
                    );
                }
            }
        }
        assert_eq!(
            to_text(buf)
                .lines()
                .nth(2)
                .unwrap_or_default()
                .chars()
                .count(),
            50
        );
    }

    /// The border character at one end of row `y` of a panel `h` rows tall.
    fn edge(y: u16, h: u16, right: bool) -> &'static str {
        match (y == 0, y + 1 == h, right) {
            (true, _, false) => "╭",
            (true, _, true) => "╮",
            (_, true, false) => "╰",
            (_, true, true) => "╯",
            _ => "│",
        }
    }

    // ------------------------------------------------------------- fixtures

    /// Every toggle, with what the screen shows for it before and after `space`.
    const TOGGLES: &[(&str, &str, &str)] = &[
        ("art", "on", "off"),
        ("progress", "on", "off"),
        ("volume", "on", "off"),
        ("popularity", "on", "off"),
        ("key_hints", "on", "off"),
        ("clock", "on", "off"),
        ("side_pane", "on", "off"),
        ("enabled", "on", "off"),
        ("mouse", "on", "off"),
        ("song_change", "off", "on"),
    ];

    /// Every choice, with what the screen shows before `→` and after it. The
    /// "after" is the second word in the type's own order, so the test fails if
    /// an enum gains a variant and the list order stops matching.
    const CHOICES: &[(&str, &str, &str)] = &[
        ("mode", "art", "visualizer"),
        ("default_tab", "history", "info"),
        ("border", "rounded", "sharp"),
        ("accent", "art", "green"),
        ("style", "spectrum", "mirrored"),
        ("source", "auto", "simulated"),
        ("control", "spotify", "system"),
        ("art_protocol", "auto", "kitty"),
    ];

    /// Every key SPEC §8's example lists, read out of the document.
    fn spec_keys() -> Vec<String> {
        let doc = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/SPEC.md"),
        )
        .expect("SPEC.md is the source of truth for this screen's keys");
        let block = doc
            .split("```toml")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .expect("SPEC §8 has a toml example in it");
        let keys: Vec<String> = block
            .lines()
            .filter(|l| !l.trim_start().starts_with('['))
            .filter_map(|l| l.split('#').next())
            .filter_map(|l| l.split_once('='))
            .map(|(k, _)| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect();
        assert!(
            keys.iter().any(|k| k == "border"),
            "the spec block was not read: {keys:?}"
        );
        keys
    }
}
