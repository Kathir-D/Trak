//! Drawing the app. Pure: it takes `&App` and writes cells. It never reads the
//! player and never changes state.
//!
//! The layout is computed from the terminal size on every frame, and the
//! breakpoints below are the ones TODO 3.4 tunes. The rule from SPEC §3 that
//! matters most: **borders must never wrap or tear**, so every pane is built from
//! `Rect`s that are shrunk before anything is drawn inside them.

use std::path::{Path, PathBuf};

use image::{DynamicImage, Rgba};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui_image::Resize;
use ratatui_image::StatefulImage;
use ratatui_image::protocol::{ImageSource, StatefulProtocol, StatefulProtocolType};

use crate::player::PlaybackState;
use crate::tui::app::{App, Control, DisplayMode, HISTORY_VIEW, Hit, Tab};
use crate::tui::theme::{Theme, format_time, progress_bar};

/// The keys in SPEC §4 that this build deliberately does not offer, and why.
///
/// Shared by the two tests that hold it to account: the help overlay must not
/// claim a key it does not have, and `update` must genuinely do nothing when one
/// of them is pressed. `#[cfg(test)]` because it exists only to be checked.
#[cfg(test)]
/// The keys in SPEC §4 that this build deliberately does not offer, and why.
///
/// The test below fails for any SPEC key that is neither in the help overlay
/// nor here, so adding a key to the SPEC cannot quietly leave the help stale
/// — and a key cannot be quietly *claimed* here either, because dropping one
/// out of this list fails the test too.
///
/// The Version A rows (`/`, `f`, `A`, `o`) are filtered out by the version
/// column, so they are not excused here: a key only needs an excuse if the
/// SPEC promises it to this build.
pub(crate) const NOT_YET: &[(&str, &str)] = &[
    (
        "4",
        "Version B has three tabs; 4-6 arrive with the Version A tabs",
    ),
    (
        "5",
        "Version B has three tabs; 4-6 arrive with the Version A tabs",
    ),
    (
        "6",
        "Version B has three tabs; 4-6 arrive with the Version A tabs",
    ),
    ("a", "the art / visualizer toggle is TODO 4.3"),
    ("v", "the visualizer style cycle is TODO 4.3"),
    ("L", "full-screen lyrics is TODO 4.4"),
    (",", "the settings screen is TODO 5.x"),
];

/// Which layout the terminal is wide enough for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout_ {
    /// Side by side: Now Playing left, tabs right.
    Wide,
    /// Stacked: the tabs pane moves under Now Playing.
    Stacked,
    /// A single strip: title, artist, progress, controls.
    Compact,
    /// Too small to draw anything sensible.
    TooSmall,
}

/// Tuned in TODO 3.4. Kept in one function so the tests and the renderer cannot
/// disagree about which layout a size gets.
pub fn layout_for(width: u16, height: u16) -> Layout_ {
    // Below this there is no honest way to draw two panes or a strip.
    if width < 30 || height < 8 {
        return Layout_::TooSmall;
    }
    // A side-by-side split needs both panes to be at least readable.
    if width >= 76 && height >= 16 {
        return Layout_::Wide;
    }
    if width >= 46 && height >= 12 {
        return Layout_::Stacked;
    }
    Layout_::Compact
}

/// The smallest art hole worth drawing. Below this the cover is a stripe, so the
/// space goes to the text instead (TODO 4.1: art disappears cleanly when the
/// terminal shrinks).
const MIN_ART: (u16, u16) = (6, 4);

/// How many rows must be left for the artist, the title, the bar, the times and
/// the controls. The cover gets whatever is left over, and the cover is what gets
/// dropped when there is not enough for both -- a TUI with no track title is not
/// a music player, a TUI with a small cover still is.
const MIN_TEXT_ROWS: u16 = 9;

/// The cell size halfblocks assume when nothing has queried the terminal.
/// TODO 1.4 measured (10, 20) in Terminal.app; a cell is about twice as tall as
/// it is wide, which is the assumption the art sizing rests on.
const HALF_BLOCK_CELL: (u16, u16) = (10, 20);

/// How much room the Now Playing pane gets, as a share of the width.
const NOW_PLAYING_SHARE: u16 = 46;

/// The image state, and the protocol it draws with.
///
/// `ratatui-image` keeps per-protocol state — for Kitty, which part of the image
/// it has already sent — so it cannot be rebuilt every frame, or the image is
/// re-sent from the top on each redraw. It lives here, owned by the event loop,
/// and the renderer only says which image goes in which rectangle.
///
/// The protocol is chosen **once**, at startup, by querying the terminal
/// (TODO 1.4: `Picker::from_query_stdio()` picked Kitty in cmux unattended, and
/// halfblocks in Terminal.app). Asking per frame would write an escape sequence
/// to the terminal on every draw.
pub struct Images {
    backend: Backend,
    protocol: Option<StatefulProtocol>,
    /// Which image the current protocol was built for: the path and the area.
    /// A resize changes the area, and Kitty's state is only valid for the size it
    /// was encoded at, so both invalidate it.
    built_for: Option<(PathBuf, u16, u16)>,
}

/// How the protocol is obtained. A picker in production, because only the
/// terminal knows what it can draw; a fixed one in tests, because a test has no
/// terminal to ask and `Picker`'s fields are private.
enum Backend {
    Fixed {
        font: (u16, u16),
        kind: StatefulProtocolType,
    },
}

impl Default for Images {
    fn default() -> Self {
        Self {
            backend: Backend::Fixed {
                font: (10, 10),
                kind: StatefulProtocolType::Halfblocks(
                    ratatui_image::protocol::halfblocks::Halfblocks::default(),
                ),
            },
            protocol: None,
            built_for: None,
        }
    }
}

impl Images {
    /// Work out what the terminal supports from the environment.
    ///
    /// **Deliberately not `Picker::from_query_stdio()`**, which asks the terminal
    /// directly and is the obvious way to do this. It starts a thread that calls
    /// `enable_raw_mode`, reads the reply from stdin, and then calls
    /// `disable_raw_mode` -- and when the terminal does not answer within its one
    /// second, that thread is still running after the TUI has re-enabled raw
    /// mode and switches it back off underneath. crossterm's event reader then
    /// sees canonical mode, so **every single keypress is silently discarded**:
    /// the interface looks alive, the clock ticks, and nothing responds to
    /// anything. That is a horrible failure and avoiding it is the entire reason
    /// this function exists.
    ///
    /// Every terminal trak cares about announces itself, and 1.4 verified what
    /// each one can actually draw. A terminal that announces nothing gets
    /// halfblocks, which every terminal can draw -- so being wrong here costs a
    /// blocky cover, not an unusable interface.
    pub fn from_terminal() -> Self {
        let env = |k: &str| std::env::var(k).unwrap_or_default();
        let (protocol, cell) =
            detect_protocol(&env("TERM_PROGRAM"), &env("TERM"), &env("KITTY_WINDOW_ID"));
        Self {
            backend: Backend::Fixed {
                font: cell,
                kind: kind_for(protocol),
            },
            protocol: None,
            built_for: None,
        }
    }

    /// A halfblocks renderer with a known cell size, for tests.
    pub fn halfblocks(cell: (u16, u16)) -> Self {
        Self {
            backend: Backend::Fixed {
                font: cell,
                kind: StatefulProtocolType::Halfblocks(
                    ratatui_image::protocol::halfblocks::Halfblocks::default(),
                ),
            },
            protocol: None,
            built_for: None,
        }
    }

    /// The cell size the terminal reports. TODO 1.4 measured (8, 17) in cmux and
    /// (10, 20) in Terminal.app; the art has to be sized in cells, not pixels.
    pub fn cell_size(&self) -> (u16, u16) {
        self.backend.cell()
    }

    /// The cell rectangle the image occupies inside `area`, keeping the aspect
    /// ratio. Square cover art in a wide, short pane is letterboxed, not
    /// stretched: a wide rectangle of a square album is not what anybody wants to
    /// look at.
    ///
    /// The arithmetic is in *cells*, because that is what the terminal draws in.
    /// A cell is roughly twice as tall as it is wide, so a square image is half
    /// as many cells across as it is down — which is why the same cover fills a
    /// tall narrow pane and letterboxes in a wide one.
    pub fn fit(&self, area: Rect, image: &DynamicImage) -> Rect {
        let (cw, ch) = self.cell_size();
        if cw == 0 || ch == 0 || area.width == 0 || area.height == 0 {
            return Rect::new(area.x, area.y, 0, 0);
        }
        let iw = f64::from(image.width().max(1));
        let ih = f64::from(image.height().max(1));
        // Cells across per cell down, for this image in this terminal.
        let ratio = (iw / ih) / (f64::from(cw) / f64::from(ch));
        // The largest rectangle of that shape that fits.
        let scale = (f64::from(area.width) / ratio).min(f64::from(area.height));
        let w = ((ratio * scale).round() as u16).clamp(1, area.width);
        let h = ((scale).round() as u16).clamp(1, area.height);
        Rect::new(
            area.x + (area.width - w) / 2,
            area.y + (area.height - h) / 2,
            w,
            h,
        )
    }

    /// Draw `image` into `area`, encoding it first if this is a new image or a
    /// new size.
    pub fn draw(&mut self, f: &mut Frame, path: &Path, image: &DynamicImage, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let sized = self.fit(area, image);
        let key = (path.to_path_buf(), sized.width, sized.height);
        if self.built_for.as_ref() != Some(&key) {
            self.protocol = Some(self.backend.protocol_for(image.clone(), sized));
            self.built_for = Some(key);
        }
        let Some(protocol) = &mut self.protocol else {
            return;
        };
        f.render_stateful_widget(
            StatefulImage::new().resize(Resize::Fit(None)),
            sized,
            protocol,
        );
    }

    /// Forget the encoded image. A resize has to call this: Kitty's state is
    /// only valid for the size it was encoded at, and drawing it at a new size
    /// either tears or draws nothing at all.
    pub fn invalidate(&mut self) {
        self.protocol = None;
        self.built_for = None;
    }

    /// Whether an image has been encoded and is ready to draw.
    pub fn is_ready(&self) -> bool {
        self.protocol.is_some()
    }
}

impl Backend {
    fn cell(&self) -> (u16, u16) {
        match self {
            Backend::Fixed { font, .. } => *font,
        }
    }

    /// Build a protocol whose image is already the right size for `area`.
    ///
    /// The resizing is done **here** rather than left to the protocol because
    /// `ImageSource::new` derives its size from the image's *natural* size and
    /// the font size, and then never grows it: a 32×32 cover with a (10, 20) cell
    /// becomes 3×2 cells, and asking for a 12×6 area draws it at 3×2. Scaling to
    /// the target rectangle first is the only way to fill the pane.
    fn protocol_for(&self, image: DynamicImage, area: Rect) -> StatefulProtocol {
        let (cw, ch) = self.cell();
        let want_w = (area.width * cw).max(1);
        let want_h = (area.height * ch).max(1);
        let scaled = image::imageops::resize(
            &image.to_rgba8(),
            u32::from(want_w),
            u32::from(want_h),
            ratatui_image::FilterType::Triangle,
        );
        let scaled = DynamicImage::ImageRgba8(scaled);
        match self {
            Backend::Fixed { font, kind } => StatefulProtocol::new(
                ImageSource::new(scaled, *font, Rgba([0, 0, 0, 0])),
                *font,
                kind.clone(),
            ),
        }
    }
}

/// The graphics protocols trak can draw with, in the order `from_terminal`
/// considers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsProtocol {
    /// Kitty and anything that implements it, including Ghostty and WezTerm.
    Kitty,
    Iterm2,
    Sixel,
    /// The fallback. Every terminal can draw it, and 1.4 measured Terminal.app
    /// rendering it correctly while printing the others as visible text.
    Halfblocks,
}

/// Work out the protocol from what the terminal says about itself.
///
/// The cell size comes from TODO 1.4's measurements rather than from a query:
/// (8, 17) in cmux, which is Ghostty, and (10, 20) in Terminal.app. Halfblocks
/// only needs the ratio to be roughly 1:2, so its cell size barely matters.
pub fn detect_protocol(
    term_program: &str,
    term: &str,
    kitty_window: &str,
) -> (GraphicsProtocol, (u16, u16)) {
    let tp = term_program.to_ascii_lowercase();
    let tm = term.to_ascii_lowercase();
    if !kitty_window.is_empty()
        || tp.contains("ghostty")
        || tp.contains("kitty")
        || tm.contains("kitty")
    {
        return (GraphicsProtocol::Kitty, (8, 17));
    }
    if tp.contains("iterm") {
        return (GraphicsProtocol::Iterm2, (10, 20));
    }
    if tp.contains("wezterm") || tm.contains("sixel") {
        return (GraphicsProtocol::Sixel, (10, 20));
    }
    (GraphicsProtocol::Halfblocks, (10, 20))
}

fn kind_for(p: GraphicsProtocol) -> StatefulProtocolType {
    match p {
        GraphicsProtocol::Kitty => StatefulProtocolType::Kitty(
            ratatui_image::protocol::kitty::StatefulKitty::new(1, false),
        ),
        GraphicsProtocol::Iterm2 => StatefulProtocolType::ITerm2(Default::default()),
        GraphicsProtocol::Sixel => StatefulProtocolType::Sixel(Default::default()),
        GraphicsProtocol::Halfblocks => StatefulProtocolType::Halfblocks(
            ratatui_image::protocol::halfblocks::Halfblocks::default(),
        ),
    }
}

/// Where the clickable things ended up, recorded as they were drawn.
///
/// The event loop hit-tests clicks against the regions the *last* frame recorded,
/// which is the only arrangement that cannot go wrong: a click is then resolved
/// against the cells the user can see, and there is no second copy of the layout
/// maths to drift out of step with the renderer.
#[derive(Debug, Default, Clone)]
pub struct Regions {
    /// Each tab label, in tab order.
    pub tabs: Vec<(Rect, usize)>,
    /// The album-art area, when the pane is big enough to show one.
    pub art: Option<Rect>,
    /// How far the title has to scroll to be read in full, or zero when it fits.
    /// The renderer knows the pane width; the loop owns the clock.
    pub title_span: usize,
    /// The history list's body, when the History tab is showing.
    pub history: Option<Rect>,
    /// The progress bar.
    pub progress: Option<Rect>,
    /// The three transport controls, left to right.
    pub controls: Vec<(Rect, Control)>,
}

impl Regions {
    /// What is at this cell, if anything.
    pub fn hit(&self, col: u16, row: u16) -> Option<Hit> {
        for (r, i) in &self.tabs {
            if r.contains((col, row).into()) {
                return Some(Hit::Tab(*i));
            }
        }
        if let Some(p) = self.progress
            && p.contains((col, row).into())
            && p.width > 0
        {
            // A seek is a fraction of the bar's width, so the position accounts
            // for where inside the bar the click landed rather than snapping to
            // the nearest whole tenth.
            let f = (col.saturating_sub(p.x) as f64 + 0.5) / p.width as f64;
            return Some(Hit::Seek(f.clamp(0.0, 1.0)));
        }
        for (r, c) in &self.controls {
            if r.contains((col, row).into()) {
                return Some(Hit::Control(*c));
            }
        }
        let h = self.history?;
        if !h.contains((col, row).into()) {
            return None;
        }
        // The first two rows of the pane are the now-playing row and the
        // heading, so the list starts at row 2 and the last two rows of a
        // short pane are not entries at all. Anything that is not an entry row
        // is still the pane, so a wheel over the heading or the gap below the
        // list scrolls rather than doing nothing.
        let entries = self.history_rows().unwrap_or(0);
        let i = row.saturating_sub(h.y + 2) as usize;
        if i >= entries {
            return Some(Hit::HistoryPane);
        }
        Some(Hit::HistoryRow(i))
    }

    /// The album-art area, for the renderer and for the tests. Not a click
    /// target: art is not interactive.
    pub fn art_area(&self) -> Option<Rect> {
        self.art
    }

    /// How many history rows are on screen, for the app to scroll by.
    pub fn history_rows(&self) -> Option<usize> {
        self.history.map(|r| (r.height as usize).saturating_sub(2))
    }
}

pub fn draw(f: &mut Frame, app: &App, theme: &Theme) {
    let mut regions = Regions::default();
    let mut images = Images::halfblocks(HALF_BLOCK_CELL);
    draw_with(f, app, theme, &mut regions, &mut images);
}

/// The renderer, and where everything clickable ended up.
pub fn draw_with(
    f: &mut Frame,
    app: &App,
    theme: &Theme,
    regions: &mut Regions,
    images: &mut Images,
) {
    *regions = Regions::default();
    let area = f.area();
    let (w, h) = (area.width, area.height);

    // The idle card replaces the whole dashboard when Spotify is not running
    // (SPEC §3). It is the only place trak ever offers to start Spotify.
    if app.is_idle() && h >= 8 && w >= 30 {
        draw_idle_card(f, area, theme);
        if app.show_help {
            draw_help(f, area);
        }
        if let Some(t) = &app.toast {
            draw_toast(f, area, &t.text, theme);
        }
        return;
    }

    match layout_for(w, h) {
        Layout_::TooSmall => draw_too_small(f, area, app),
        Layout_::Compact => draw_compact(f, area, app, theme, regions, images),
        Layout_::Stacked => draw_stacked(f, area, app, theme, regions, images),
        Layout_::Wide => draw_wide(f, area, app, theme, regions, images),
    }

    if app.show_help {
        draw_help(f, area);
    }
    if let Some(t) = &app.toast {
        draw_toast(f, area, &t.text, theme);
    }
}

/// Shrink a pane by one cell on every side so its border cannot touch the pane
/// next to it, which is what makes borders look torn.
fn inner(r: Rect) -> Rect {
    if r.width < 2 || r.height < 2 {
        return Rect::ZERO;
    }
    Rect {
        x: r.x + 1,
        y: r.y + 1,
        width: r.width - 2,
        height: r.height - 2,
    }
}

/// A bordered pane with a title. The title may be styled per span, which is how
/// the selected tab gets the accent background (TODO 4.2).
fn pane_block<'a>(title: impl Into<Line<'a>>, theme: &Theme, focused: bool) -> Block<'a> {
    let mut b = Block::bordered()
        .title(title.into())
        .border_type(
            theme
                .border
                .to_ratatui()
                .unwrap_or(ratatui::widgets::BorderType::Rounded),
        )
        .border_style(if focused {
            theme.accent_style()
        } else {
            Theme::dim()
        });
    if focused {
        b = b.title_style(theme.accent_style().add_modifier(Modifier::BOLD));
    }
    b
}

fn draw_wide(
    f: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    regions: &mut Regions,
    images: &mut Images,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(0),    // body
            Constraint::Length(1), // footer
        ])
        .split(area);

    draw_header(f, rows[0], app, theme);

    if !app.settings.side_pane {
        draw_now_playing(f, rows[1], app, theme, regions, images);
    } else {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(NOW_PLAYING_SHARE),
                Constraint::Min(24),
            ])
            .split(rows[1]);
        draw_now_playing(f, cols[0], app, theme, regions, images);
        draw_tabs(f, cols[1], app, theme, regions);
    }

    draw_footer(f, rows[2], app, theme);
}

fn draw_stacked(
    f: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    regions: &mut Regions,
    images: &mut Images,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Percentage(58),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(f, rows[0], app, theme);
    draw_now_playing(f, rows[1], app, theme, regions, images);
    if app.settings.side_pane {
        draw_tabs(f, rows[2], app, theme, regions);
    }
    draw_footer(f, rows[3], app, theme);
}

/// The idle card: Spotify is not running, and enter will launch it.
///
/// Deliberately does not say what trak is about to do beyond that, and never
/// launches on its own (COMPAT rule 2).
fn draw_idle_card(f: &mut Frame, area: Rect, theme: &Theme) {
    let card_w = (area.width * 2 / 3).clamp(34, 64);
    let card_h = 9.min(area.height);
    let card = Rect {
        x: area.x + area.width.saturating_sub(card_w) / 2,
        y: area.y + area.height.saturating_sub(card_h) / 2,
        width: card_w,
        height: card_h,
    };
    f.render_widget(Clear, card);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "Spotify isn't running",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "press enter to launch it in the background",
                Style::default().fg(Color::Cyan),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "trak never starts Spotify on its own.",
                Theme::dim(),
            )),
            Line::from(Span::styled("q  quit            ?  keys", Theme::dim())),
        ])
        .alignment(Alignment::Center)
        .block(
            Block::bordered()
                .title(" trak ")
                .border_type(
                    theme
                        .border
                        .to_ratatui()
                        .unwrap_or(ratatui::widgets::BorderType::Rounded),
                )
                .border_style(theme.accent_style()),
        ),
        card,
    );
}

fn draw_compact(
    f: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    regions: &mut Regions,
    _images: &mut Images,
) {
    let Some(track) = app.track() else {
        draw_too_small(f, area, app);
        return;
    };
    let lines = vec![
        Line::from(vec![
            Span::styled(
                app.state
                    .as_ref()
                    .map(|s| s.playback.symbol())
                    .unwrap_or("▶"),
                Style::default().fg(Theme::status_colour(app.state.as_ref().map(|s| s.playback))),
            ),
            Span::raw(" "),
            Span::styled(&track.title, Style::default().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(Span::styled(track.artist.clone(), Theme::dim())),
        Line::from(Span::styled(
            progress_bar(progress(app), area.width.saturating_sub(4) as usize),
            theme.accent_style(),
        )),
    ];
    f.render_widget(Paragraph::new(lines), area);
    regions.progress = Some(Rect {
        x: area.x + 2,
        y: area.y + 2,
        width: area.width.saturating_sub(4),
        height: 1,
    });
}

fn draw_too_small(f: &mut Frame, area: Rect, app: &App) {
    // SPEC §3: say the terminal is too small rather than drawing something torn.
    let msg = if area.width < 24 || area.height < 4 {
        "too small".to_string()
    } else if app.is_idle() {
        "Spotify isn't running\n\n  press enter to launch\n  q to quit".to_string()
    } else {
        "terminal too small — resize".to_string()
    };
    f.render_widget(
        Paragraph::new(msg)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_header(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let state = app.state.as_ref().map(|s| s.playback);
    let mut left = vec![Span::styled(
        " trak ",
        Style::default()
            .fg(theme.palette.end())
            .add_modifier(Modifier::BOLD),
    )];
    if !app.is_idle() {
        // The dot breathes while music plays. A static dot says something is
        // playing; one that pulses says it is playing.
        left.push(Span::styled(
            crate::tui::theme::status_dot(app.is_playing(), app.tick_secs),
            Style::default().fg(Theme::status_colour(state)),
        ));
        left.push(Span::raw(" "));
        left.push(Span::styled(
            state.map(|s| s.symbol()).unwrap_or("▶"),
            theme.accent_style(),
        ));
        left.push(Span::raw("  "));
        if let Some(t) = app.track() {
            let room = (area.width as usize).saturating_sub(14);
            left.push(Span::styled(
                crate::tui::theme::marquee(&t.title, room, app.marquee_offset),
                Style::default().add_modifier(Modifier::BOLD),
            ));
        }
    } else {
        left.push(Span::styled("not running", Theme::dim()));
    }

    let right = if app.settings.show_clock && !app.clock.is_empty() {
        vec![Span::styled(
            format!("{} ", app.clock),
            Style::default()
                .fg(theme.palette.primary)
                .add_modifier(Modifier::DIM),
        )]
    } else {
        vec![]
    };

    f.render_widget(Paragraph::new(Line::from(left)), area);
    if !right.is_empty() {
        f.render_widget(Paragraph::new(Line::from(right)).right_aligned(), area);
    }
}

fn draw_now_playing(
    f: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    regions: &mut Regions,
    images: &mut Images,
) {
    let block = pane_block(" Now Playing ", theme, true);
    f.render_widget(block, area);
    let body = inner(area);
    if body.width == 0 || body.height == 0 {
        return;
    }

    let Some(track) = app.track() else {
        f.render_widget(
            Paragraph::new("nothing is playing").wrap(Wrap { trim: true }),
            body,
        );
        return;
    };

    let pos = app.interpolated_position();
    let dur = track.duration_secs() as f64;
    let muted_style = app.is_idle();
    let mut lines: Vec<Line> = Vec::new();
    let mut art_lines = 0u16;

    // The cover owns the top of the pane. It is drawn large and hard against the
    // left edge rather than centred in a frame: the artwork *is* the visual, and
    // a box around it says "placeholder here" when nothing could be more certain.
    // The cover takes 45% of the pane, but never at the cost of the text: on a
    // short pane the cover goes and the title stays.
    let art_h = (body.height * 55 / 100)
        .min(body.height.saturating_sub(MIN_TEXT_ROWS))
        .clamp(0, 20);
    let hole_w = body.width;
    let hole_h = art_h;
    if hole_h >= MIN_ART.1 && hole_w >= MIN_ART.0 {
        // The spine occupies the first two columns, so the cover is inset past
        // it. Drawing the spine over the artwork instead looks like a rendering
        // fault rather than a design choice.
        let hole = Rect {
            x: body.x + 2,
            y: body.y,
            width: hole_w.saturating_sub(2),
            height: hole_h,
        };
        // The visualizer takes the same rectangle (TODO 4.3). The cover is still
        // fetched and still drives the accent: a visualizer tinted by the album
        // is the entire point of having one.
        let show_visualizer = app.settings.display_mode == DisplayMode::Visualizer;
        let drawn = app
            .art_enabled
            .then(|| app.art.drawable(track))
            .flatten()
            .filter(|_| app.art.error.is_none());
        if show_visualizer {
            draw_visualizer_placeholder(f, hole, app, theme);
        } else if let Some(art) = drawn {
            f.render_widget(ratatui::widgets::Clear, hole);
            images.draw(f, &art.path, &art.image, hole);
        } else if app.art.loading {
            let hint = Rect { height: 1, ..hole };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "fetching the cover…",
                    Style::default().add_modifier(Modifier::ITALIC),
                )))
                .alignment(Alignment::Center),
                hint,
            );
        }
        regions.art = Some(hole);
        // A gradient spine down the left edge of the artwork, tying the cover to
        // the type underneath it. Two cells, and it is the difference between a
        // pane that looks assembled and one that looks designed.
        let spine_lines = crate::tui::theme::spine(hole_h as usize, &theme.palette);
        for (i, span) in spine_lines.into_iter().enumerate() {
            f.render_widget(
                Paragraph::new(Line::from(span)),
                Rect {
                    x: hole.x - 2,
                    y: hole.y + i as u16,
                    width: 2,
                    height: 1,
                },
            );
        }
        art_lines = hole_h;
    }

    // Everything below the cover is text, so the row arithmetic below is relative
    // to this rectangle: measuring from `body` is off by the height of the art,
    // which is how the progress bar ends up not matching the thing you click.
    let text_body = Rect {
        y: body.y + art_lines,
        height: body.height.saturating_sub(art_lines),
        ..body
    };
    if text_body.height == 0 {
        return;
    }
    let dim = Theme::dim();
    let title_w = text_body.width as usize;

    if !track.artist.is_empty() {
        lines.push(Line::from(Span::styled(
            crate::tui::theme::spaced_caps(&track.artist),
            dim,
        )));
    }
    // The title scrolls rather than truncating or wrapping: a chorus you cannot
    // read is worse than one that moves.
    regions.title_span = crate::tui::theme::marquee_span(&track.title, title_w);
    let title = crate::tui::theme::marquee(&track.title, title_w, app.marquee_offset);
    lines.push(crate::tui::theme::gradient_line(&title, &theme.palette));
    // Single releases put the track name in the album field too; printing it
    // twice looks like a bug.
    if !track.album.is_empty() && !track.album.eq_ignore_ascii_case(&track.title) {
        lines.push(Line::from(Span::styled(
            crate::tui::theme::marquee(&track.album, title_w, app.marquee_offset / 2),
            Style::default()
                .fg(theme.accent_colour())
                .add_modifier(Modifier::DIM),
        )));
    }
    lines.push(Line::from(""));

    // The bar, as a gradient with a bright head, so the eye finds the position
    // off the colour as well as the length.
    let bar_row = text_body.y + lines.len() as u16;
    lines.push(Line::from(crate::tui::theme::gradient_bar(
        progress(app),
        title_w,
        &theme.palette,
        !app.is_playing(),
    )));
    regions.progress = Some(Rect {
        x: text_body.x,
        y: bar_row,
        width: text_body.width,
        height: 1,
    });

    let elapsed = format_time(pos);
    let dur_str = format_time(dur);
    let mut times = vec![Span::styled(elapsed.clone(), dim)];
    // Push the duration to the right edge rather than leaving a gap that the
    // reader has to measure.
    let used = elapsed.chars().count();
    if text_body.width as usize > used + dur_str.chars().count() + 2 {
        times.push(Span::raw(
            " ".repeat(text_body.width as usize - used - dur_str.chars().count()),
        ));
    }
    times.push(Span::styled(
        dur_str,
        Style::default().fg(theme.accent_colour()),
    ));
    lines.push(Line::from(times));
    lines.push(Line::from(""));

    // Controls, with the shuffle and repeat badges next to them.
    let ctl_row = text_body.y + lines.len() as u16;
    let controls = vec![
        Span::raw(" "),
        Span::styled("⏮", Style::default().fg(theme.accent_colour())),
        Span::raw(" "),
        Span::styled(
            match app.state.as_ref().map(|s| s.playback) {
                Some(PlaybackState::Playing) => "⏸",
                _ => "▶",
            },
            Style::default()
                .fg(theme.palette.end())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled("⏭", Style::default().fg(theme.accent_colour())),
    ];
    let mut badges = String::new();
    if app.state.as_ref().is_some_and(|s| s.shuffling_enabled) {
        badges.push_str("   ⇄");
    }
    if app.repeat != crate::player::RepeatMode::Off {
        badges.push_str("   ");
        badges.push_str(app.repeat.symbol());
    }
    let mut control_line = controls;
    control_line.push(Span::raw(badges));
    lines.push(Line::from(control_line));

    // The three transport glyphs sit at columns 1, 3 and 5 of that line. Their
    // exact cell width is ambiguous (ratatui and the terminal can disagree about
    // ⏮ and ⏸), so the targets are given two cells each.
    regions.controls = [(0, Control::Prev), (2, Control::Toggle), (4, Control::Next)]
        .into_iter()
        .map(|(dx, c)| {
            (
                Rect {
                    x: text_body.x + dx,
                    y: ctl_row,
                    width: 2,
                    height: 1,
                },
                c,
            )
        })
        .collect();

    // The volume meter gets the same treatment as the bar, because it is the same
    // kind of thing: a quantity, drawn with the album's own colours. Hidden when
    // Spotify ignored a write (COMPAT rule 5).
    if app.settings.show_volume && !app.volume_hidden {
        let v = app.meter_volume();
        let label = "vol ";
        let meter_w = (text_body.width as usize)
            .saturating_sub(label.len() + 5)
            .max(1);
        let mut row = vec![Span::styled(label, dim)];
        row.extend(crate::tui::theme::gradient_meter(
            f64::from(v) / 100.0,
            meter_w,
            &theme.palette,
            app.muted,
        ));
        row.push(Span::raw(" "));
        row.push(Span::styled(
            format!("{v:>3}%"),
            Style::default().fg(theme.accent_colour()),
        ));
        lines.push(Line::from(row));
    }

    let shown = lines.len().min(text_body.height as usize);
    if shown > 0 {
        f.render_widget(
            Paragraph::new(lines.into_iter().take(shown).collect::<Vec<_>>()),
            text_body,
        );
    }

    // A gradient rule along the bottom of the pane. On a tall terminal there is
    // always space left over, and a bare gap at the foot of a panel reads as an
    // oversight; a rule reads as a deliberate edge, and it is the last thing the
    // album's colours get to say.
    let spare = text_body.height.saturating_sub(shown as u16);
    if spare >= 2 {
        let rule_y = body.y + body.height - 1;
        f.render_widget(
            Paragraph::new(crate::tui::theme::gradient_line(
                &"─".repeat(text_body.width as usize),
                &theme.palette,
            ))
            .alignment(Alignment::Center),
            Rect {
                x: text_body.x,
                y: rule_y,
                width: text_body.width,
                height: 1,
            },
        );
    }
    let _ = muted_style;
}

fn progress(app: &App) -> f64 {
    match &app.state {
        Some(s) => s.progress(),
        None => 0.0,
    }
}

/// Where the real visualizer goes (TODO 8.1). Until then this draws its frame,
/// its style name and a flat spectrum of bars in the album's own colours, so the
/// toggle visibly swaps the pane rather than leaving a hole in the layout.
fn draw_visualizer_placeholder(f: &mut Frame, hole: Rect, app: &App, theme: &Theme) {
    if hole.width == 0 || hole.height == 0 {
        return;
    }
    f.render_widget(ratatui::widgets::Clear, hole);
    let name = app.settings.visualizer_style.label();
    if hole.height < 4 || hole.width < 12 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(name, Theme::dim()))),
            hole,
        );
        return;
    }
    // A row of bars, sized from the interpolated position so even the placeholder
    // moves with the music rather than sitting there inert.
    let ramp = theme.palette.ramp(hole.width as usize);
    let bars = (hole.width as usize / 2).max(1);
    let mut row: Vec<Span> = Vec::with_capacity(bars);
    for i in 0..bars {
        let phase =
            (i as f64 / bars as f64 * std::f64::consts::TAU) + app.interpolated_position() * 0.6;
        let height = 0.5 + 0.5 * phase.sin().abs();
        let cells = ((height * (hole.height - 2) as f64).round() as usize).clamp(1, 4);
        let colour = ramp[(i * 2) % ramp.len()];
        let top = hole.y + ((hole.height as usize - cells) / 2) as u16;
        for c in 0..cells {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled("▄", Style::default().fg(colour)))),
                Rect {
                    x: hole.x + (i * 2) as u16,
                    y: top + c as u16,
                    width: 1,
                    height: 1,
                },
            );
        }
        row.push(Span::raw(" "));
    }
    let _ = row;
    // The style name, dimmed, at the top of the pane: it says what is being drawn
    // and what `v` will change.
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            name,
            Theme::dim().add_modifier(Modifier::ITALIC),
        ))),
        Rect {
            x: hole.x,
            y: hole.y,
            width: hole.width,
            height: 1,
        },
    );
}

fn draw_tabs(f: &mut Frame, area: Rect, app: &App, theme: &Theme, regions: &mut Regions) {
    let selected_style = Style::default()
        .fg(theme.accent_text())
        .bg(theme.accent_colour());
    // The segments come first: the title is built from them and the clickable
    // rects come from them, so a tab cannot be drawn in one place and clicked
    // somewhere else.
    let strip = tab_strip(app, selected_style);
    let block = pane_block(strip.line(), theme, false);
    f.render_widget(block, area);
    let body = inner(area);
    if body.width == 0 || body.height == 0 {
        return;
    }

    // The strip is drawn as the pane's *title*, and ratatui draws a title over
    // the top border rather than inside the body. So the row is the pane's own
    // first row, not the body's, and the title is " <strip> ", which puts the
    // strip two cells in from the pane's left edge.
    let strip_x = area.x + 2;
    let strip_y = area.y;
    for (i, (offset, label)) in strip.segments.iter().enumerate() {
        let w = label.chars().count() as u16;
        regions.tabs.push((
            Rect {
                x: strip_x + offset,
                y: strip_y,
                width: w,
                height: 1,
            },
            i,
        ));
    }

    let lines: Vec<Line> = match app.tab {
        Tab::History => {
            regions.history = Some(body);
            history_lines(app, theme)
        }
        Tab::Info => info_lines(app, theme),
        Tab::Lyrics => lyrics_lines(app, theme),
    };
    f.render_widget(Paragraph::new(lines), body);
}

/// The Lyrics tab: the line being sung, the ones coming, and the ones just gone.
///
/// Synced lyrics are the whole reason this tab exists, so the current line is set
/// in the album's own gradient and everything else recedes -- dimmer, and further
/// back. Four lines above and a dozen below is what fits a pane without turning
/// it into a wall of text.
fn lyrics_lines<'a>(app: &App, theme: &'a Theme) -> Vec<Line<'a>> {
    use crate::tui::app::LyricsStatus;

    let dim = Theme::dim();
    let wrap_hint = |t: &str| {
        vec![
            Line::from(Span::styled(t.to_string(), dim)),
            Line::from(""),
            Line::from(Span::styled(
                "no lyrics for this one",
                dim.add_modifier(Modifier::ITALIC),
            )),
            Line::from(Span::styled(
                "LRCLIB has most songs but not all of them",
                dim,
            )),
        ]
    };

    match &app.lyrics.status {
        LyricsStatus::Idle | LyricsStatus::Loading => {
            let title = app.track().map(|t| t.title.clone()).unwrap_or_default();
            vec![
                Line::from(Span::styled(
                    title,
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    if app.is_idle() {
                        ""
                    } else {
                        "looking for lyrics…"
                    },
                    dim.add_modifier(Modifier::ITALIC),
                )),
            ]
        }
        LyricsStatus::NotFound => wrap_hint("no lyrics for this one"),
        LyricsStatus::Failed(why) => {
            let mut out = vec![Line::from(Span::styled(
                why.clone(),
                dim.add_modifier(Modifier::ITALIC),
            ))];
            out.push(Line::from(""));
            out.push(Line::from(Span::styled("try again on the next track", dim)));
            out
        }
        LyricsStatus::Ready => {
            let Some(lyrics) = app.lyrics.lyrics.as_ref() else {
                return wrap_hint("nothing to show");
            };
            if lyrics.instrumental {
                return wrap_hint("this one is instrumental");
            }
            if lyrics.lines.is_empty() {
                return wrap_hint("no lyrics for this one");
            }
            let pos = app.interpolated_position();
            let active = lyrics
                .lines
                .iter()
                .rposition(|l| !l.time_secs.is_nan() && l.time_secs <= pos)
                .unwrap_or(0);
            // Only synced lyrics have a position; unsynced ones are shown from
            // the top, which is all that can honestly be done with them.
            let synced = lyrics.synced && !lyrics.lines[active].time_secs.is_nan();

            let mut out: Vec<Line> = Vec::new();
            // A few lines of lead-in, dimmer the further back they are.
            let from = active.saturating_sub(4);
            for (i, line) in lyrics.lines[from..=active].iter().enumerate() {
                let back = active - (from + i);
                let style = if back == 0 && synced {
                    Style::default()
                } else {
                    dim.add_modifier(Modifier::DIM)
                };
                out.push(if back == 0 && synced {
                    crate::tui::theme::gradient_line(&line.text, &theme.palette)
                } else {
                    Line::from(Span::styled(line.text.clone(), style))
                });
            }
            // And the ones coming.
            for line in lyrics.lines.iter().skip(active + 1).take(14) {
                out.push(Line::from(Span::styled(
                    line.text.clone(),
                    dim.add_modifier(Modifier::DIM),
                )));
            }
            if !lyrics.synced {
                out.push(Line::from(""));
                out.push(Line::from(Span::styled("unsynced lyrics", dim)));
            }
            out
        }
    }
}

/// The tab strip, as text plus the pieces it is made of.
///
/// Built once and used twice — for the pane title and for the clickable rects —
/// because a tab strip rendered from one description and clicked from another is
/// a tab strip that eventually stops lining up.
struct TabStrip {
    /// Cell offset from the start of the title, and the text, per tab.
    segments: Vec<(u16, String)>,
    spans: Vec<Span<'static>>,
}

impl TabStrip {
    /// The title, leading space included, as one line of styled spans.
    fn line(&self) -> Line<'static> {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(self.spans.iter().cloned());
        spans.push(Span::raw(" "));
        Line::from(spans)
    }
}

fn tab_strip(app: &App, selected: Style) -> TabStrip {
    let mut segments = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut width = 0usize;
    for (i, t) in Tab::ALL.iter().enumerate() {
        let n = i + 1;
        let (label, style) = if *t == app.tab {
            (format!("[{n}]{}", t.label()), selected)
        } else {
            (format!(" {n} {} ", t.label()), Style::default())
        };
        if width > 0 {
            spans.push(Span::raw(" "));
            width += 1;
        }
        segments.push((width as u16, label.clone()));
        width += label.chars().count();
        spans.push(Span::styled(label, style));
    }
    TabStrip { segments, spans }
}

fn history_lines<'a>(app: &'a App, theme: &'a Theme) -> Vec<Line<'a>> {
    let mut out = Vec::new();

    // The track that is playing is not in the history yet -- history holds the
    // *outgoing* tracks -- so it is drawn as its own row above them, marked with
    // the same ▶ the SPEC uses, and it is not selectable. Otherwise the current
    // song is missing from the tab you use to remember what you just played.
    if let Some(t) = app.track() {
        let now = if t.artist.is_empty() {
            t.title.clone()
        } else {
            format!("{} — {}", t.artist, t.title)
        };
        out.push(Line::from(Span::styled(
            format!("▶ {now}"),
            theme.accent_style().add_modifier(Modifier::BOLD),
        )));
    }

    if app.history.is_empty() {
        if out.is_empty() {
            out.push(Line::from(Span::styled(
                "nothing yet this session — tracks appear here as they play",
                Theme::dim(),
            )));
        } else {
            out.push(Line::from(Span::styled(
                "  no earlier tracks this session",
                Theme::dim(),
            )));
        }
        return out;
    }

    out.push(Line::from(Span::styled(
        "  earlier this session",
        Theme::dim(),
    )));

    // Newest first, which is what a history is for, and the same order the
    // cursor counts in (`App::selected_history`).
    out.extend(
        app.visible_history()
            .take(HISTORY_VIEW)
            .enumerate()
            .map(|(i, e)| {
                let selected = i == app.history_cursor;
                let marker = if selected { "›" } else { " " };
                let text = if e.track.artist.is_empty() {
                    e.track.title.clone()
                } else {
                    format!("{} — {}", e.track.artist, e.track.title)
                };
                let style = if selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                Line::from(Span::styled(format!("{marker} {text}"), style))
            }),
    );
    out
}

fn info_lines<'a>(app: &'a App, theme: &'a Theme) -> Vec<Line<'a>> {
    let Some(t) = app.track().cloned() else {
        return vec![Line::from(Span::styled("nothing is playing", Theme::dim()))];
    };
    // 18 is the longest label ("heard this session"); a narrower column runs the
    // value straight into the label with no gap.
    let dash = |k: &str, v: String| -> Line<'a> {
        Line::from(vec![
            Span::styled(format!("{k:<18} "), Theme::dim()),
            Span::styled(v, theme.accent_style()),
        ])
    };

    let mut out = vec![
        dash("title", t.title.clone()),
        dash("artist", or_dash(&t.artist)),
        dash("album", or_dash(&t.album)),
        dash("album artist", or_dash(&t.album_artist)),
    ];
    out.push(dash("duration", format_time(t.duration_secs() as f64)));
    out.push(dash("track", t.track_number.to_string()));
    out.push(dash("disc", t.disc_number.to_string()));
    // Optional means "Spotify did not tell us", which for an advert is a true
    // answer and must print as a dash rather than a 0 (TODO 3.7).
    out.push(dash(
        "popularity",
        t.popularity
            .map(|p| p.to_string())
            .unwrap_or_else(|| "—".into()),
    ));
    out.push(dash(
        "play count",
        t.play_count
            .map(|p| p.to_string())
            .unwrap_or_else(|| "—".into()),
    ));
    // Only the track with a URI can be counted, so an advert prints a dash
    // rather than a number it could not have earned.
    out.push(dash(
        "heard this session",
        app.times_heard()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".into()),
    ));
    out.push(dash("uri", or_dash(t.uri.as_deref().unwrap_or(""))));
    out.push(dash(
        "artwork",
        t.artwork_url
            .as_deref()
            .map(|_| "yes".into())
            .unwrap_or_else(|| "—".into()),
    ));
    if t.is_ad() {
        out.push(Line::from(Span::styled(
            "this is an advert — no track link",
            Theme::dim(),
        )));
    }
    out
}

fn or_dash(s: &str) -> String {
    if s.is_empty() {
        "—".into()
    } else {
        s.to_string()
    }
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    // The headless hint is more important than a key list when the Dock icon is
    // gone, so it takes the left-hand space and the keys move right (TODO 4.7).
    if let Some(hint) = app.headless.hint()
        && area.width as usize > hint.chars().count() + 24
    {
        let left = Rect {
            width: (area.width as usize - 1 - hint.chars().count()) as u16,
            ..area
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {hint} "),
                Style::default()
                    .fg(theme.palette.primary)
                    .add_modifier(Modifier::ITALIC),
            ))),
            left,
        );
        let rest = Rect {
            x: area.x + left.width,
            width: area.width - left.width,
            ..area
        };
        draw_footer_keys(f, rest, app, theme);
        return;
    }
    draw_footer_keys(f, area, app, theme);
}

fn draw_footer_keys(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if !app.settings.show_key_hints {
        f.render_widget(Paragraph::new(""), area);
        return;
    }
    let hints: Vec<(&str, &str)> = if app.is_idle() {
        vec![("enter", "launch"), ("q", "quit")]
    } else {
        vec![
            ("space", "play/pause"),
            ("n/p", "next/prev"),
            ("←/→", "seek"),
            ("+/-", "vol"),
            ("s", "shuffle"),
            ("r", "repeat"),
            ("c", "copy"),
            ("?", "help"),
            ("q", "quit"),
        ]
    };
    let spans: Vec<Span> = hints
        .iter()
        .map(|(k, v)| Span::styled(format!(" {k} {v}"), theme.accent_style()))
        .collect();
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// How wide the key column of the help overlay is. The keys are what the SPEC
/// parity test reads, so the width is a constant rather than a `{:>16}` buried
/// in a format string.
const HELP_KEY_WIDTH: usize = 16;

/// Where the help overlay sits. Shared with the parity test so it reads the same
/// cells the renderer wrote instead of guessing.
fn help_popup(area: Rect) -> Rect {
    // A centred overlay. Clear first so it reads as a panel over the app.
    let w = (area.width * 3 / 5).clamp(30, 60);
    let h = (area.height * 3 / 5).clamp(9, 24);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

fn draw_help(f: &mut Frame, area: Rect) {
    let popup = help_popup(area);
    f.render_widget(Clear, popup);

    // Only keys that do something today. Listing one that is not bound yet
    // would be a small lie, and `the_help_overlay_matches_the_spec_table` fails
    // the build if a key here is not either bound or listed in SPEC §4.
    let rows = vec![
        line("space", "play / pause"),
        line("n / p", "next / previous track"),
        line("h / l  ← →", "seek back / forward"),
        line("+ / -", "volume up / down"),
        line("m", "mute (saves the volume you had)"),
        line("s", "toggle shuffle"),
        line("r", "repeat: off → all → one     R  replay"),
        line("c", "copy the share link"),
        line("j / k  ↑ ↓", "move in a list"),
        line("enter", "play the selected item"),
        line("tab", "next tab    shift-tab  back"),
        line("1 2 3", "jump to a tab"),
        line("? / esc", "close this"),
        line("q / ctrl-c", "quit"),
    ];

    f.render_widget(
        Paragraph::new(rows)
            .block(
                Block::bordered()
                    .title(" Keys ")
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn line(k: &str, v: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{k:<HELP_KEY_WIDTH$}"),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(v.to_string()),
    ])
}

fn draw_toast(f: &mut Frame, area: Rect, text: &str, theme: &Theme) {
    // Bottom right, one line, and never over the footer keys (TODO 4.8).
    let w = (text.chars().count() as u16 + 4).min(area.width);
    if w < 8 || area.height < 2 {
        return;
    }
    let popup = Rect {
        x: area.x + area.width.saturating_sub(w),
        y: area.y + area.height.saturating_sub(2),
        width: w,
        height: 1,
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {text} "),
            toast_style(theme),
        ))),
        popup,
    );
}

/// The toast sits on the accent, so its text colour has to follow the accent too
/// (TODO 4.2: a contrast rule that holds for every colour the cover can produce).
fn toast_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.accent_text())
        .bg(theme.accent_colour())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::parse::parse;
    use crate::testutil::fixture;
    use crate::tui::app::Event;
    use crate::tui::app::update;

    fn app_at(w: u16, h: u16) -> App {
        let _ = (w, h);
        update(
            App::new(),
            Event::PlayerState(Box::new(parse(&fixture("playing_track.txt")).unwrap())),
        )
        .app
    }

    /// The breakpoints, pinned so the renderer and the tests cannot drift.
    #[test]
    fn breakpoints_pick_the_documented_layout() {
        assert_eq!(layout_for(100, 30), Layout_::Wide);
        assert_eq!(layout_for(76, 16), Layout_::Wide);
        assert_eq!(layout_for(75, 30), Layout_::Stacked);
        assert_eq!(layout_for(60, 20), Layout_::Stacked);
        assert_eq!(layout_for(46, 12), Layout_::Stacked);
        assert_eq!(layout_for(45, 20), Layout_::Compact);
        assert_eq!(layout_for(30, 8), Layout_::Compact);
        assert_eq!(layout_for(29, 30), Layout_::TooSmall);
        assert_eq!(layout_for(100, 7), Layout_::TooSmall);
    }

    /// Every size must land in exactly one layout, and a size never straddles a
    /// boundary into a torn layout.
    #[test]
    fn every_size_gets_exactly_one_layout() {
        for w in (10..=200u16).step_by(7) {
            for h in (4..=60u16).step_by(3) {
                let l = layout_for(w, h);
                // monotonic: a wider terminal is never a smaller layout
                if w >= 76 && h >= 16 {
                    assert_eq!(l, Layout_::Wide, "at {w}x{h}");
                }
            }
        }
    }

    /// The real test: render at many sizes and check the frame is still whole.
    #[test]
    fn rendering_never_panics_at_any_size() {
        for w in [20u16, 30, 45, 46, 60, 75, 76, 100, 200] {
            for h in [4u16, 7, 8, 12, 16, 30, 60] {
                let backend = ratatui::backend::TestBackend::new(w, h);
                let mut term = ratatui::Terminal::new(backend).unwrap();
                let app = app_at(w, h);
                let theme = Theme::default();
                let rendered = term.draw(|f| draw(f, &app, &theme));
                assert!(rendered.is_ok(), "draw failed at {w}x{h}");
            }
        }
    }

    /// TODO 3.5: quitting Spotify must show the idle card, and it must be the
    /// only place trak offers to launch it.
    #[test]
    fn the_idle_card_appears_and_offers_a_launch() {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let theme = Theme::default();
        term.draw(|f| draw(f, &App::new(), &theme)).unwrap();

        let buf = term.backend().buffer().clone();
        let text = (0..30)
            .map(|y| {
                (0..100)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("isn't running"), "{text}");
        assert!(text.contains("press enter to launch"), "{text}");
        assert!(text.contains("never starts Spotify on its own"), "{text}");
        // the dashboard is replaced, not drawn underneath
        assert!(!text.contains("Now Playing"), "{text}");
    }

    #[test]
    fn the_idle_card_disappears_once_a_track_arrives() {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let app = app_at(100, 30);
        let theme = Theme::default();
        term.draw(|f| draw(f, &app, &theme)).unwrap();
        let buf = term.backend().buffer().clone();
        let text = (0..30)
            .map(|y| {
                (0..100)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("press enter to launch"), "{text}");
        assert!(text.contains("Now Playing"), "{text}");
    }

    #[test]
    fn rendering_works_with_nothing_playing() {
        for (w, h) in [(80u16, 24u16), (50, 14), (35, 10), (25, 6)] {
            let backend = ratatui::backend::TestBackend::new(w, h);
            let mut term = ratatui::Terminal::new(backend).unwrap();
            let theme = Theme::default();
            term.draw(|f| draw(f, &App::new(), &theme)).unwrap();
        }
    }

    /// The help overlay's text, checked as a snapshot. The `?` overlay must list

    #[test]
    fn the_help_overlay_is_centred_and_does_not_fill_the_screen() {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let mut app = app_at(100, 30);
        app.show_help = true;
        let theme = Theme::default();
        term.draw(|f| draw(f, &app, &theme)).unwrap();
        let buf = term.backend().buffer().clone();
        // The app is still visible above the overlay, so it is a panel not a page.
        let first_row: String = (0..100).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(
            first_row.contains("trak"),
            "the app is still drawn: {first_row:?}"
        );
    }

    #[test]
    fn rendering_works_with_the_help_overlay_and_a_toast() {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let mut app = app_at(100, 30);
        app.show_help = true;
        app.toast = Some(crate::tui::app::Toast {
            text: "copied https://open.spotify.com/track/abc".into(),
            at: std::time::Instant::now(),
        });
        let theme = Theme::default();
        term.draw(|f| draw(f, &app, &theme)).unwrap();
    }

    #[test]
    fn the_help_overlay_renders_at_every_size_that_has_a_layout() {
        for (w, h) in [(100u16, 30u16), (80, 24), (60, 18), (50, 14), (35, 10)] {
            let backend = ratatui::backend::TestBackend::new(w, h);
            let mut term = ratatui::Terminal::new(backend).unwrap();
            let mut app = app_at(w, h);
            app.show_help = true;
            let theme = Theme::default();
            term.draw(|f| draw(f, &app, &theme)).unwrap();
        }
    }

    /// The SPEC §3 layout, checked as a snapshot at the size it was drawn for.
    #[test]
    fn the_wide_layout_looks_like_the_spec() {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let app = app_at(100, 30);
        let theme = Theme::default();
        term.draw(|f| draw(f, &app, &theme)).unwrap();

        let buf = term.backend().buffer().clone();
        let text = (0..30)
            .map(|y| {
                (0..100)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Now Playing"), "{text}");
        assert!(text.contains("History"), "the tab strip: {text}");
        assert!(text.contains("Census Designated"), "{text}");
        assert!(text.contains("play/pause"), "the footer hints: {text}");
        // rounded borders are the default (SPEC §2)
        assert!(text.contains('╭'), "rounded border: {text}");
        assert!(text.contains('╰'), "rounded border: {text}");
    }

    #[test]
    fn a_tiny_terminal_says_so_rather_than_drawing_a_torn_border() {
        let backend = ratatui::backend::TestBackend::new(24, 6);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let app = app_at(24, 6);
        let theme = Theme::default();
        term.draw(|f| draw(f, &app, &theme)).unwrap();
        let buf = term.backend().buffer().clone();
        let text = (0..6)
            .map(|y| {
                (0..24)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("too small") || text.contains("small"),
            "{text}"
        );
    }

    #[test]
    fn the_compact_strip_shows_title_artist_and_a_bar() {
        let backend = ratatui::backend::TestBackend::new(40, 10);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let app = app_at(40, 10);
        let theme = Theme::default();
        term.draw(|f| draw(f, &app, &theme)).unwrap();
        let buf = term.backend().buffer().clone();
        let text = (0..10)
            .map(|y| {
                (0..40)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Census Designated"), "{text}");
        assert!(text.contains("Jane Remover"), "{text}");
        assert!(text.contains('●') || text.contains('─'), "a bar: {text}");
    }

    #[test]
    fn the_info_tab_prints_a_dash_for_what_spotify_did_not_say() {
        // An advert: no uri, no popularity, no artwork. It must not print zeros.
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let mut app = update(
            App::new(),
            Event::PlayerState(Box::new(parse(&fixture("playing_ad.txt")).unwrap())),
        )
        .app;
        app.tab = Tab::Info;
        let theme = Theme::default();
        term.draw(|f| draw(f, &app, &theme)).unwrap();
        let buf = term.backend().buffer().clone();
        let text = (0..30)
            .map(|y| {
                (0..100)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains('—'), "missing values print a dash: {text}");
        assert!(text.contains("advert"), "{text}");
    }

    /// The Info tab is where "times heard this session" lives (TODO 3.7).
    #[test]
    fn the_info_tab_counts_the_plays_this_session() {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let mut app = app_at(100, 30);
        app.tab = Tab::Info;
        // Hear the same track again, so the count is not just the trivial one.
        let mut other = parse(&fixture("playing_track.txt")).unwrap();
        other.track.uri = Some("spotify:track:OTHER".into());
        other.track.title = "Other".into();
        app = update(app, Event::PlayerState(Box::new(other))).app;
        let census = parse(&fixture("playing_track.txt")).unwrap();
        app = update(app, Event::PlayerState(Box::new(census))).app;

        let theme = Theme::default();
        term.draw(|f| draw(f, &app, &theme)).unwrap();
        let buf = term.backend().buffer().clone();
        let text = (0..30)
            .map(|y| {
                (0..100)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("heard this session"), "{text}");
        assert!(
            text.contains("heard this session 2"),
            "twice this session: {text}"
        );
    }

    /// The current track is drawn above the history, marked ▶, because the
    /// history itself only holds the tracks that have already finished.
    #[test]
    fn the_history_tab_marks_the_track_that_is_playing() {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let mut app = app_at(100, 30);
        app.tab = Tab::History;
        assert!(
            text_of(&mut term, &app, 100, 30).contains("▶ Jane Remover — Census Designated"),
            "the now-playing row"
        );

        let mut other = parse(&fixture("playing_track.txt")).unwrap();
        other.track.title = "Something Else".into();
        // A different URI, because history records by URI: a different title on
        // the same track is not a change.
        other.track.uri = Some("spotify:track:OTHER".into());
        app = update(app, Event::PlayerState(Box::new(other))).app;
        let text = text_of(&mut term, &app, 100, 30);
        assert!(text.contains("▶ Jane Remover — Something Else"), "{text}");
        assert!(
            text.contains("earlier this session"),
            "the finished tracks go under a heading: {text}"
        );
        assert!(
            text.contains("Census Designated"),
            "kept as history: {text}"
        );
    }

    /// With nothing played yet there is no ▶ row, and the tab says so.
    #[test]
    fn an_empty_history_says_so() {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let app = app_at(100, 30);
        let text = text_of(&mut term, &app, 100, 30);
        assert!(text.contains("no earlier tracks"), "{text}");
    }

    /// COMPAT rule 5: a meter that Spotify ignores is hidden rather than shown
    /// wrong.
    #[test]
    fn the_volume_meter_is_hidden_when_spotify_ignored_the_write() {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let mut app = app_at(100, 30);
        // The meter glyph, not the word "vol": the footer hint says "+/- vol".
        assert!(text_of(&mut term, &app, 100, 30).contains("vol "));
        app.volume_hidden = true;
        let text = text_of(&mut term, &app, 100, 30);
        assert!(
            !text.contains('▰'),
            "a meter that cannot be trusted: {text}"
        );
        assert!(!text.contains("100%"), "{text}");
    }

    /// The meter shows the user's own choice, which a poll cannot move.
    #[test]
    fn the_meter_shows_the_users_own_volume() {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let mut app = app_at(100, 30);
        // A read reports 100; the user has asked for 40.
        app.user_volume = Some(40);
        let text = text_of(&mut term, &app, 100, 30);
        assert!(text.contains("40%"), "{text}");
        assert!(!text.contains("100%"), "not the raw read: {text}");
    }

    /// Every key the SPEC promises for Version B, read out of the SPEC itself.
    ///
    /// Parsed from `docs/SPEC.md` rather than copied into a list, because a
    /// copied list is a list that goes stale: this is the check that the help
    /// overlay cannot drift from the table it claims to mirror.
    fn spec_keys() -> Vec<String> {
        let spec = include_str!("../../docs/SPEC.md");
        let section = spec
            .split_once("\n## 4. Keys")
            .expect("SPEC §4 exists")
            .1
            .split_once("\n## 5.")
            .expect("SPEC §5 follows §4")
            .0;
        let mut keys = Vec::new();
        for row in section.lines().filter(|l| l.starts_with("|")) {
            let cells: Vec<&str> = row.trim_matches('|').split("|").map(str::trim).collect();
            // | key | action | version |
            if cells.len() != 3 || cells[0] == "Key" || set_contains(cells[0], &["---"]) {
                continue;
            }
            // Only the keys this build promises. The Version A rows are not
            // trak's problem until Version A exists.
            if cells[2] != "both" {
                continue;
            }
            for cell in cells[0].split('`') {
                for token in cell.split('/') {
                    // `Shift-Tab` is the same key as `Tab`, but `L` is not `l`,
                    // so the shift- prefix goes and the case stays.
                    let t = token.trim();
                    let t = t
                        .strip_prefix("shift-")
                        .or_else(|| t.strip_prefix("Shift-"))
                        .unwrap_or(t);
                    // A range like 1–6 only needs its ends checked. The en dash
                    // only; a plain '-' is part of the key (`ctrl-c`).
                    for end in t.split('\u{2013}') {
                        let e = end.trim();
                        if !e.is_empty() {
                            keys.push(e.to_string());
                        }
                    }
                }
            }
        }
        keys.sort();
        keys.dedup();
        keys
    }

    fn set_contains(haystack: &str, needles: &[&str]) -> bool {
        needles.iter().all(|n| haystack.contains(n))
    }

    /// SPEC §4 says "keep this table and the `?` help overlay in sync". This is
    /// that check, and it reads the SPEC rather than a copy of it.
    #[test]
    fn the_help_overlay_matches_the_spec_table() {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let mut app = app_at(100, 30);
        app.show_help = true;
        // Tall enough for every row the overlay has.
        let theme = Theme::default();
        term.draw(|f| draw(f, &app, &theme)).unwrap();
        // Only the overlay's key column, never the whole screen: a one-letter
        // key such as `m` appears somewhere on any 100x30 screen, so searching
        // the whole buffer would pass no matter what the overlay said.
        let popup = help_popup(Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 30,
        });
        let buf = term.backend().buffer().clone();
        let rows: Vec<String> = (0..popup.height)
            .map(|i| {
                let y = popup.y + 1 + i;
                (popup.x + 1..popup.x + 1 + HELP_KEY_WIDTH as u16)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .filter(|row| !row.trim().is_empty())
            .collect();
        // Whitespace tokens, compared exactly and case-sensitively. A substring
        // search is not good enough in either direction: `a` is "inside" `tab`,
        // and `L` is not the same key as `l`.
        let help: Vec<&str> = rows
            .iter()
            .flat_map(|r| r.split_whitespace())
            .filter(|t| *t != "/")
            .collect();

        let spec = spec_keys();
        assert!(
            spec.len() > 10,
            "the SPEC table should have parsed: {spec:?}"
        );
        let mut missing = Vec::new();
        let help_lower: Vec<String> = help.iter().map(|t| t.to_lowercase()).collect();
        for key in spec {
            let in_help = help_lower.contains(&key.to_lowercase());
            let excused = NOT_YET.iter().any(|(k, _)| *k == key);
            assert!(
                in_help || excused,
                "SPEC §4 lists `{key}` and the help overlay neither offers it nor \
                 excuses it in NOT_YET"
            );
            if !in_help {
                missing.push(key);
            }
        }
        assert!(
            missing.iter().all(|k| NOT_YET.iter().any(|(n, _)| n == k)),
            "every missing key must be excused: {missing:?}"
        );

        // And the other direction: an excuse for a key the help *does* offer is
        // stale, and would let a key quietly fall out of the help later.
        for (key, why) in NOT_YET {
            assert!(
                !help.contains(key),
                "NOT_YET excuses `{key}` ({why}) but the help overlay offers it"
            );
        }
    }

    /// Draw a frame and hand back both the cells and where things landed, which
    /// is the only way to test a click against what is really on screen.
    fn render(w: u16, h: u16, app: &App) -> (ratatui::buffer::Buffer, Regions) {
        let backend = ratatui::backend::TestBackend::new(w, h);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let mut regions = Regions::default();
        let mut images = Images::halfblocks(HALF_BLOCK_CELL);
        let theme = Theme::default();
        term.draw(|f| draw_with(f, app, &theme, &mut regions, &mut images))
            .unwrap();
        (term.backend().buffer().clone(), regions)
    }

    /// The cells of one row, by column. Indexing the buffer rather than slicing
    /// a joined string: a glyph like ⏮ is three bytes, and slicing by column
    /// lands mid-character.
    fn row_text(buf: &ratatui::buffer::Buffer, y: u16, from: u16, len: u16) -> String {
        (from..from + len)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect()
    }

    /// A click has to land where the pixels are. This is the test that says so:
    /// it finds the progress bar *in the rendered text*, clicks the middle of
    /// it, and checks the hit is the middle of the track.
    #[test]
    fn clicking_the_rendered_progress_bar_seeks_to_that_point() {
        let app = app_at(100, 30);
        let (buf, regions) = render(100, 30, &app);
        let bar = regions.progress.expect("a progress bar");
        // The bar's own columns, not the whole row: the pane's border is in the
        // way and the bar is drawn with ─ ─ rather than ▰ ▰ until it advances.
        let row = row_text(&buf, bar.y, bar.x, bar.width);
        assert_eq!(row.chars().count(), 44, "the bar spans the pane's width");
        assert!(
            row.chars().all(|c| matches!(c, '─' | '▰' | '█')),
            "the bar should be drawn where the region says: {row:?}"
        );

        // A third of the way along the bar.
        let x = bar.x + bar.width / 3;
        let hit = regions.hit(x, bar.y).expect("the bar is clickable");
        match hit {
            Hit::Seek(f) => {
                assert!(
                    (f - 1.0 / 3.0).abs() < 0.05,
                    "a third of the way along must seek to a third, got {f}"
                );
            }
            other => panic!("expected a seek, got {other:?}"),
        }
        // The two ends, which is where off-by-one errors live. A click resolves
        // to the middle of the cell it landed in, so the ends are half a cell in
        // rather than exactly 0 and 1.
        let half = 0.5 / bar.width as f64;
        let near = |want: f64, got: Hit| match got {
            Hit::Seek(f) => assert!((f - want).abs() < 1e-9, "wanted {want}, got {f}"),
            other => panic!("expected a seek, got {other:?}"),
        };
        near(half, regions.hit(bar.x, bar.y).expect("the left end"));
        near(
            1.0 - half,
            regions
                .hit(bar.x + bar.width - 1, bar.y)
                .expect("the right end"),
        );
        // One past the end of the bar is not a seek at all.
        assert_eq!(regions.hit(bar.x + bar.width, bar.y), None);
    }

    /// Every tab label the strip draws has to be clickable, and clicking one
    /// selects it. The rects come from the same segments as the title.
    #[test]
    fn every_tab_is_clickable_where_it_is_drawn() {
        let app = app_at(100, 30);
        let (buf, regions) = render(100, 30, &app);
        assert_eq!(regions.tabs.len(), Tab::ALL.len());
        for (i, tab) in Tab::ALL.iter().enumerate() {
            let (r, idx) = regions.tabs[i];
            assert_eq!(idx, i);
            let drawn = row_text(&buf, r.y, r.x, r.width);
            assert!(
                drawn.contains(tab.label()),
                "tab {i} region does not sit over its label: {drawn:?}"
            );
        }
        // And a click on the second tab switches to it.
        let (r, _) = regions.tabs[1];
        let mut next = app.clone();
        next = update(
            next,
            Event::Mouse(crate::tui::app::Mouse {
                action: crate::tui::app::MouseAction::Press,
                target: regions.hit(r.x + 1, r.y).unwrap(),
            }),
        )
        .app;
        assert_eq!(next.tab, Tab::Info);
    }

    /// The three transport controls, and that they are on the row the renderer
    /// drew them on.
    #[test]
    fn the_transport_controls_are_clickable() {
        let app = app_at(100, 30);
        let (buf, regions) = render(100, 30, &app);
        assert_eq!(regions.controls.len(), 3);
        let want = [Control::Prev, Control::Toggle, Control::Next];
        for (i, (r, c)) in regions.controls.iter().enumerate() {
            assert_eq!(*c, want[i]);
            let drawn = row_text(&buf, r.y, r.x, r.width);
            assert!(
                ["⏮", "⏸", "▶", "⏹", "⏭"].iter().any(|g| drawn.contains(*g)),
                "control {i} is not over a transport glyph: {drawn:?}"
            );
            assert_eq!(regions.hit(r.x, r.y), Some(Hit::Control(want[i])));
        }
    }

    /// A click on a history row selects that row, counted from what is shown.
    #[test]
    fn clicking_a_history_row_selects_it() {
        let mut app = app_at(100, 30);
        for i in 0..5 {
            let mut s = parse(&fixture("playing_track.txt")).unwrap();
            s.track.uri = Some(format!("spotify:track:t{i}"));
            s.track.title = format!("Old {i}");
            app = update(app, Event::PlayerState(Box::new(s))).app;
        }
        let (_, regions) = render(100, 30, &app);
        let h = regions.history.expect("the history body");
        // Row 0 of the list is the third line of the body: the now-playing row
        // and the heading come first.
        let row = h.y + 2;
        assert_eq!(regions.hit(h.x + 2, row), Some(Hit::HistoryRow(0)));
        assert_eq!(regions.hit(h.x + 2, row + 2), Some(Hit::HistoryRow(2)));
        // Below the last row is the pane, not a row: a wheel there still scrolls.
        let past = h.y + h.height + 5;
        assert_eq!(
            regions.hit(h.x + 2, past),
            None,
            "outside the pane entirely"
        );
    }

    /// The wheel needs a target that is not a row, and one that is not a row
    /// *or* the pane would be a dead zone.
    #[test]
    fn a_wheel_over_the_history_has_somewhere_to_land() {
        let app = app_at(100, 30);
        let (_, regions) = render(100, 30, &app);
        let h = regions.history.expect("the history body");
        for row in h.y..h.y + h.height {
            assert!(
                matches!(
                    regions.hit(h.x, row),
                    Some(Hit::HistoryRow(_)) | Some(Hit::HistoryPane)
                ),
                "row {row} is not scrollable"
            );
        }
    }

    /// Nothing outside the dashboard is clickable, and a frame with no tab pane
    /// records no regions rather than stale ones.
    #[test]
    fn an_idle_card_has_nothing_to_click() {
        let (buf, regions) = render(100, 30, &App::new());
        let screen: String = (0..30)
            .map(|y| row_text(&buf, y, 0, 100))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(screen.contains("isn't running"), "{screen}");
        // The card replaces the whole dashboard, so there is nothing clickable
        // at all -- not even the tab strip.
        assert!(regions.tabs.is_empty(), "the card has no tabs");
        assert!(regions.history.is_none(), "there is no list to click");
        assert!(regions.progress.is_none(), "and no bar to scrub");
        assert!(regions.controls.is_empty());
    }

    /// A hit test must never divide by a zero-width bar or produce a fraction
    /// outside 0..=1.
    #[test]
    fn a_degenerate_region_is_never_a_target() {
        let mut regions = Regions {
            progress: Some(Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            }),
            ..Regions::default()
        };
        assert_eq!(regions.hit(0, 0), None, "a zero-width bar is not a target");
        regions.progress = Some(Rect::new(10, 0, 4, 1));
        assert_eq!(regions.hit(10, 0), Some(Hit::Seek(0.125)));
        // A history region too short to hold the two header rows is all pane.
        let mut regions = Regions {
            history: Some(Rect::new(0, 0, 10, 2)),
            ..Regions::default()
        };
        assert_eq!(regions.hit(0, 0), Some(Hit::HistoryPane));
        assert_eq!(regions.hit(0, 1), Some(Hit::HistoryPane));
        regions.history = Some(Rect::new(0, 0, 10, 5));
        assert_eq!(regions.hit(0, 2), Some(Hit::HistoryRow(0)));
        assert_eq!(regions.history_rows(), Some(3));
    }

    /// A small, deterministic image, written to a temporary file so
    /// `StatefulImage` has something to load. Halfblocks paint background
    /// colours into cells, so a real gradient is what makes the snapshot mean
    /// something.
    /// Red and blue blocks, not a smooth gradient.
    ///
    /// A gradient downsamples into a handful of distinct colours and the
    /// assertion ends up counting noise. Two colours that cannot be confused with
    /// each other make the test say what it means: the image's *content* reached
    /// the buffer.
    fn fixture_image(dir: &Path) -> PathBuf {
        use image::{Rgb, RgbImage};
        let mut img = RgbImage::new(32, 32);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = if (x / 4 + y / 4) % 2 == 0 {
                Rgb([220, 20, 20])
            } else {
                Rgb([20, 20, 220])
            };
        }
        let path = dir.join("fixture.png");
        img.save(&path).expect("writing the fixture");
        path
    }

    /// A temporary directory for one test.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "trak-render-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// TODO 4.1's done-when: a snapshot of the half-block path.
    ///
    /// Halfblocks are the fallback that every terminal can draw, and the only
    /// protocol a `TestBackend` can show — Kitty draws by writing escape sequences
    /// to the terminal, which a buffer cannot show. So this is the only art test
    /// that can exist at all, and it is the one that matters for Terminal.app.
    #[test]
    fn the_half_block_path_actually_draws_the_image() {
        let dir = temp_dir("art");
        let path = fixture_image(&dir);
        let image = crate::art::decode(&path).expect("decoding the fixture");

        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let mut app = app_at(100, 30);
        let url = app.track().unwrap().artwork_url.clone().unwrap();
        assert!(app.art.begin(&url));
        let mut next = app.clone();
        next = update(
            next,
            Event::Art {
                url,
                result: Ok(crate::player::actions::LoadedArt {
                    path: path.clone(),
                    image: image.clone(),
                }),
            },
        )
        .app;

        let mut regions = Regions::default();
        let mut images = Images::halfblocks(HALF_BLOCK_CELL);
        let theme = Theme::default();
        term.draw(|f| draw_with(f, &next, &theme, &mut regions, &mut images))
            .unwrap();

        let art = regions.art.expect("an art area when there is an image");
        let buf = term.backend().buffer().clone();
        // The fixture is a gradient, so the art area must contain more than one
        // colour, and every cell in it must be painted. A blank area would mean
        // the protocol drew nothing and the frame is a lie.
        // Halfblocks draw each cell as a foreground/background pair, so "painted"
        // means a half-block glyph with a colour -- not a particular one of the
        // two, which is what an earlier version of this test got wrong.
        let mut reddish = 0u32;
        let mut bluish = 0u32;
        let mut glyphs = std::collections::HashSet::new();
        for y in art.y..art.y + art.height {
            for x in art.x..art.x + art.width {
                let cell = &buf[(x, y)];
                glyphs.insert(cell.symbol());
                for c in [cell.fg, cell.bg] {
                    let ratatui::style::Color::Rgb(r, _, b) = c else {
                        continue;
                    };
                    if r > 120 && r > b + 60 {
                        reddish += 1;
                    } else if b > 120 && b > r + 60 {
                        bluish += 1;
                    }
                }
            }
        }
        assert!(
            reddish > 0 && bluish > 0,
            "both halves of the fixture must reach the buffer: {reddish} red, {bluish} blue"
        );
        assert!(
            glyphs.contains("▀"),
            "halfblocks use the upper-half-block glyph: {glyphs:?}"
        );

        // And the rest of the dashboard is still there, below the image: the text
        // block starts under the art rather than being painted over by it. The
        // artist is drawn in spaced small caps, so match on the track title.
        let below: String = (art.y + art.height..art.y + art.height + 5)
            .map(|y| row_text(&buf, y, 0, 46))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            below.contains("Census"),
            "the title block must be below the art:\n{below}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Resizing must throw the encoded image away: Kitty's state is only valid
    /// for the size it was encoded at, and reusing it draws nothing or draws it
    /// at the wrong scale.
    #[test]
    fn a_resize_invalidates_the_encoded_image() {
        let mut images = Images::halfblocks(HALF_BLOCK_CELL);
        assert!(!images.is_ready());
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::new(8, 8));
        let backend = ratatui::backend::TestBackend::new(60, 20);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let path = PathBuf::from("/tmp/whatever.png");
        term.draw(|f| images.draw(f, &path, &img, f.area()))
            .unwrap();
        assert!(images.is_ready(), "the first draw encodes it");

        images.invalidate();
        assert!(!images.is_ready(), "and a resize throws that away");
    }

    /// The image is sized in *cells*, keeping the aspect ratio: a wide, short
    /// pane must letterbox a square cover rather than stretch it.
    #[test]
    fn the_art_is_letterboxed_to_its_aspect_ratio() {
        let images = Images::halfblocks(HALF_BLOCK_CELL);
        let square = image::DynamicImage::ImageRgb8(image::RgbImage::new(10, 10));
        let pane = Rect::new(0, 0, 40, 8);
        let fitted = images.fit(pane, &square);
        assert!(fitted.width <= pane.width && fitted.height <= pane.height);
        // The property that matters is the *pixel* aspect: square cover art must
        // come out square, which in cells means about twice as wide as it is
        // tall, because a cell is twice as tall as it is wide.
        let (cw, ch) = images.cell_size();
        let px_w = f64::from(fitted.width * cw);
        let px_h = f64::from(fitted.height * ch);
        assert!(
            (px_w / px_h - 1.0).abs() < 0.05,
            "square art must not be stretched: {fitted:?} is {px_w}x{px_h}px"
        );
        assert!(
            fitted.width < pane.width,
            "and it must be letterboxed rather than filling the pane: {fitted:?}"
        );
        // Centred, so the leftover space is split.
        assert_eq!(fitted.x, (pane.width - fitted.width) / 2);
        assert!(fitted.width > 0 && fitted.height > 0);
    }

    /// A degenerate area must not divide by zero or produce a negative rect.
    #[test]
    fn fitting_into_nothing_produces_nothing() {
        let images = Images::halfblocks(HALF_BLOCK_CELL);
        let square = image::DynamicImage::ImageRgb8(image::RgbImage::new(10, 10));
        // No area at all means no rectangle at all.
        for pane in [Rect::new(0, 0, 0, 0), Rect::new(5, 5, 0, 10)] {
            let fitted = images.fit(pane, &square);
            assert_eq!((fitted.width, fitted.height), (0, 0), "{pane:?}");
        }
        // One cell is not nothing, and must not come out as a negative or
        // overflowing rect.
        for pane in [Rect::new(0, 0, 1, 1), Rect::new(3, 3, 2, 1)] {
            let fitted = images.fit(pane, &square);
            assert!(fitted.width >= 1 && fitted.width <= pane.width, "{pane:?}");
            assert!(
                fitted.height >= 1 && fitted.height <= pane.height,
                "{pane:?}"
            );
        }
    }

    /// No image yet means the placeholder frame, and the space is reserved either
    /// way, so the layout does not jump when the cover arrives.
    #[test]
    fn the_art_space_is_reserved_before_the_image_arrives() {
        let app = app_at(100, 30);
        let (buf, regions) = render(100, 30, &app);
        let art = regions.art.expect("the art area is reserved");
        assert!(art.width > 0 && art.height > 0);
        // The placeholder is a frame of box-drawing characters.
        // The hole sits inside the Now Playing pane, so the pane's rounded border
        // is the frame around it: two cells above the hole, not one.
        // The spine sits beside the cover, two cells at the pane's left edge, and
        // the hole starts after it.
        assert_eq!(art.x, 3, "the hole is inset past the spine: {art:?}");
        let spine = row_text(&buf, art.y, art.x - 2, 2);
        assert_eq!(spine, "██", "the spine is two cells of gradient");
        // The hole is otherwise untouched until a cover arrives.
        let row = row_text(&buf, art.y, art.x, art.width);
        assert_eq!(row.trim(), "", "the hole should be empty: {row:?}");
        assert!(art.width >= MIN_ART.0 && art.height >= MIN_ART.1, "{art:?}");
        // And it is empty, because there is no image yet: the space is reserved,
        // not filled with a picture of nothing.
        // The spine occupies the first two columns; everything right of it is
        // untouched, because there is no cover yet.
        let hole: String = (0..art.height)
            .map(|i| row_text(&buf, art.y + i, art.x + 2, art.width - 2))
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(hole.trim(), "", "the hole should be empty: {hole:?}");
    }

    /// TODO 4.1: "art disappears cleanly when the terminal shrinks below the
    /// breakpoint". A small terminal must draw the compact strip, with no art
    /// area and nothing left behind.
    #[test]
    fn the_art_disappears_cleanly_on_a_small_terminal() {
        let app = app_at(100, 30);
        for (w, h) in [(46, 12), (30, 8), (29, 30), (100, 7)] {
            let (buf, regions) = render(w, h, &app);
            assert!(
                regions.art.is_none(),
                "{w}x{h} is too small for art but reserved {}",
                regions
                    .art_area()
                    .map(|r| format!("{r:?}"))
                    .unwrap_or_default()
            );
            let text = (0..h)
                .map(|y| row_text(&buf, y, 0, w))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                !text.contains('┌') || text.contains("too small"),
                "{w}x{h} should not draw an art frame:\n{text}"
            );
        }
    }

    fn text_of(
        term: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
        app: &App,
        w: u16,
        h: u16,
    ) -> String {
        let theme = Theme::default();
        term.draw(|f| draw(f, app, &theme)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_sharp_border_setting_changes_the_glyphs() {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let app = app_at(100, 30);
        let theme = Theme::new(
            crate::tui::theme::Accent::Green,
            crate::tui::theme::Border::Sharp,
        );
        term.draw(|f| draw(f, &app, &theme)).unwrap();
        let buf = term.backend().buffer().clone();
        let text = (0..30)
            .map(|y| {
                (0..100)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains('┌'), "sharp border: {text}");
        assert!(!text.contains('╭'), "must not be rounded: {text}");
    }

    #[test]
    fn inner_shrinks_so_borders_never_touch() {
        let r = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 4,
        };
        let i = inner(r);
        assert_eq!((i.x, i.y), (1, 1));
        assert_eq!((i.width, i.height), (8, 2));
        // and it never panics on a degenerate rect
        assert_eq!(
            inner(Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1
            }),
            Rect::ZERO
        );
        assert_eq!(
            inner(Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0
            }),
            Rect::ZERO
        );
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::{GraphicsProtocol, detect_protocol};

    /// The owner's terminal, and the one 1.4 measured. If this changes, 1.4's
    /// conclusions change with it.
    #[test]
    fn cmux_is_recognised_as_kitty() {
        let (p, cell) = detect_protocol("ghostty", "xterm-ghostty", "");
        assert_eq!(p, GraphicsProtocol::Kitty);
        assert_eq!(cell, (8, 17), "1.4 measured (8, 17) in cmux");
    }

    #[test]
    fn the_known_terminals_all_land_somewhere_sensible() {
        for (tp, term, want) in [
            ("ghostty", "xterm-ghostty", GraphicsProtocol::Kitty),
            ("WezTerm", "xterm-256color", GraphicsProtocol::Sixel),
            ("iTerm.app", "xterm-256color", GraphicsProtocol::Iterm2),
            (
                "Apple_Terminal",
                "xterm-256color",
                GraphicsProtocol::Halfblocks,
            ),
            ("", "xterm-kitty", GraphicsProtocol::Kitty),
            ("", "", GraphicsProtocol::Halfblocks),
            (
                "something-unheard-of",
                "xterm-256color",
                GraphicsProtocol::Halfblocks,
            ),
        ] {
            assert_eq!(
                detect_protocol(tp, term, "").0,
                want,
                "TERM_PROGRAM={tp:?} TERM={term:?}"
            );
        }
        // KITTY_WINDOW_ID is set by kitty itself and is the most direct signal.
        assert_eq!(
            detect_protocol("", "xterm-256color", "1").0,
            GraphicsProtocol::Kitty
        );
    }

    /// Halfblocks is the only protocol that works everywhere, so anything
    /// unrecognised must land there rather than on a guess that would draw
    /// escape sequences as visible text (1.4).
    #[test]
    fn an_unknown_terminal_never_gets_a_protocol_that_would_print_escape_codes() {
        for (tp, term) in [
            ("Alacritty", "alacritty"),
            ("hyper", "xterm-256color"),
            ("contour", "xterm-256color"),
            ("", "linux"),
        ] {
            assert_eq!(
                detect_protocol(tp, term, "").0,
                GraphicsProtocol::Halfblocks,
                "{tp:?}/{term:?}"
            );
        }
    }

    #[test]
    fn every_halfblocks_cell_size_is_about_one_to_two() {
        // The art is sized in cells, and halfblocks packs two vertical pixels per
        // cell, so a wildly wrong cell size stretches the cover.
        let (_, cell) = detect_protocol("", "xterm-256color", "");
        let ratio = f64::from(cell.1) / f64::from(cell.0);
        assert!(
            (1.5..=2.5).contains(&ratio),
            "a cell should be about twice as tall as it is wide: {cell:?}"
        );
    }
}
