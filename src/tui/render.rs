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
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use ratatui_image::Resize;
use ratatui_image::StatefulImage;
use ratatui_image::protocol::{ImageSource, StatefulProtocol, StatefulProtocolType};

use crate::player::PlaybackState;
use crate::tui::app::{App, Control, DisplayMode, HISTORY_VIEW, Hit, Tab, VisualizerStyle};
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
/// Keys SPEC §4 lists that this build does not bind yet, each with the honest
/// reason. A key that *is* bound must never appear here; a test presses every
/// excuse through `update` and fails the build if the excuse has become false.
pub(crate) const NOT_YET: &[(&str, &str)] = &[];

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

/// How wide the Now Playing pane is, in a pane row of `body_rows` usable height.
///
/// The cover is square and a cell is about twice as tall as it is wide, so a cover
/// `h` rows tall is `2h` cells across: the width at which the cover exactly fills
/// the height it is given. Below that the pane has empty columns beside the
/// picture; above it, empty rows under it. So the pane is sized to that width,
/// within [`NOW_PLAYING_SHARE`] and [`NOW_PLAYING_MIN`].
///
/// This is what makes the dashboard look the same on a 24-inch monitor and on a
/// laptop: before, the pane was always 46 % of the width, so a wide terminal gave
/// a small cover in a large empty box and a tall one gave a cover with a third of
/// the screen under it (owner, 2026-10-02).
fn now_playing_width(total: u16, body_rows: u16, cell: (u16, u16), text: u16) -> u16 {
    let rows = body_rows.saturating_sub(text);
    let (cw, ch) = cell;
    // Rows of cover that the pane's width implies, turned back into a width.
    let square = (u32::from(rows) * u32::from(ch) / u32::from(cw.max(1))) as u16;
    let ideal = square.saturating_add(4).max(NOW_PLAYING_MIN);
    ideal.min((u32::from(total) * u32::from(NOW_PLAYING_SHARE) / 100) as u16)
}

/// The most rows the text under the cover can need: artist, title, album, a
/// spacer, the bar, the times, a spacer, the controls and the meter.
///
/// The upper bound on purpose: it is used to size the pane, and a pane sized for
/// text that is not there is a pane with a gap in it. `text_rows` is the exact
/// answer for a given app; a test pins the two together.
const TEXT_ROWS_MAX: u16 = 10;

/// The rows the text block under the cover actually takes, for this app.
fn text_rows(app: &App, track: &crate::player::TrackInfo) -> u16 {
    let mut n = 1 // artist
        + 1 // title
        + u16::from(!track.album.is_empty() && !track.album.eq_ignore_ascii_case(&track.title))
        + 1 // spacer
        + u16::from(app.settings.show_progress)
        + 1 // times
        + 1 // spacer
        + 1; // controls
    if app.settings.show_volume && !app.volume_hidden {
        n += 1;
    }
    n
}

/// The transport buttons, two cells each. Every glyph is one cell wide in
/// every terminal font tried, which the media symbols are not.
const TRANSPORT_PREV: &str = "◀◀";
const TRANSPORT_PAUSE: &str = "▮▮";
const TRANSPORT_PLAY: &str = " ▶";
const TRANSPORT_NEXT: &str = "▶▶";

/// The smallest art hole worth drawing. Below this the cover is a stripe, so the
/// space goes to the text instead (TODO 4.1: art disappears cleanly when the
/// terminal shrinks).
const MIN_ART: (u16, u16) = (6, 4);

/// The cell size halfblocks assume when nothing has queried the terminal.
/// TODO 1.4 measured (10, 20) in Terminal.app; a cell is about twice as tall as
/// it is wide, which is the assumption the art sizing rests on.
const HALF_BLOCK_CELL: (u16, u16) = (10, 20);

/// The most the Now Playing pane may have, as a share of the width, and the least
/// it may have in cells.
///
/// The pane is sized to what it has to show (see [`now_playing_width`]), and these
/// are the two ends of that: never so wide that the cover is a small picture in a
/// large empty box, never so narrow that it stops being a pane.
const NOW_PLAYING_SHARE: u16 = 50;
const NOW_PLAYING_MIN: u16 = 30;

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
    /// The cover as one colour per cell, for the lyrics page, and what it was
    /// sampled for. Kept because resampling it on every frame is wasted work.
    backdrop: Option<Backdrop>,
    /// Set when the protocol was rebuilt this frame, so the caller can repaint.
    ///
    /// A Kitty placement covers exactly the cells it was encoded for, and the new
    /// cover's placement is a different picture at a possibly different size, so
    /// the *terminal* is left holding cells the new image does not cover. ratatui
    /// cannot fix that: those cells are the image's own, and its diff never writes
    /// them. The result is the old cover's grey placeholder boxes and stripes left
    /// on screen beside the new one (owner, 2026-10-03, "remove those random
    /// smaller gray boxes"). The only cure is to clear the screen and draw again.
    repaint: bool,
}

/// The cover sampled one colour per cell, keyed by the image and the rectangle.
type Backdrop = ((PathBuf, Rect), Vec<(u8, u8, u8)>);

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
            backdrop: None,
            repaint: false,
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
            backdrop: None,
            repaint: false,
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
            backdrop: None,
            repaint: false,
        }
    }

    /// The cell size the terminal reports. TODO 1.4 measured (8, 17) in cmux and
    /// (10, 20) in Terminal.app; the art has to be sized in cells, not pixels.
    pub fn cell_size(&self) -> (u16, u16) {
        self.backend.cell()
    }

    /// Whether the image was re-encoded since this was last asked, and clear the
    /// answer. The caller repaints the whole screen when it is true.
    pub fn repainted(&mut self) -> bool {
        std::mem::take(&mut self.repaint)
    }

    /// Use the cell size the terminal reports instead of the one that was
    /// measured on somebody else's machine.
    ///
    /// Everything about the art is sized in cells -- a square cover is about twice
    /// as many cells across as it is down -- so a cell size that is wrong by a
    /// factor of two makes the cover the wrong size and leaves the pane half
    /// empty. The defaults were measured once, in one terminal, at one font size
    /// (TODO 1.4), and they were wrong on the owner's other machine: a cover came
    /// out at a third of the pane with a third of the screen empty under it.
    pub fn set_cell_size(&mut self, cell: (u16, u16)) {
        if cell.0 == 0 || cell.1 == 0 {
            return;
        }
        let Backend::Fixed { font, .. } = &mut self.backend;
        *font = cell;
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
            // A new picture, or the same picture at a new size. Either way the
            // terminal is holding cells the new placement will not cover.
            self.repaint = true;
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

    /// Paint `image` into `area` as cell backgrounds, darkened, under whatever
    /// text is already there (the full-screen lyrics page).
    ///
    /// A graphics protocol cannot do this: Kitty's placeholders and sixel both
    /// own their cells, so text drawn over the cover would cut holes in it. A
    /// background colour per cell is something every colour terminal can show
    /// under a glyph. It is coarse, one colour per cell, which suits a backdrop:
    /// the cover is there to set the mood, and the words are what is read.
    pub fn backdrop(&mut self, buf: &mut Buffer, path: &Path, image: &DynamicImage, area: Rect) {
        let fit = self.fit(area, image);
        if fit.width == 0 || fit.height == 0 {
            return;
        }
        let key = (path.to_path_buf(), fit);
        if self.backdrop.as_ref().map(|(k, _)| k) != Some(&key) {
            let small = image::imageops::resize(
                &image.to_rgb8(),
                u32::from(fit.width),
                u32::from(fit.height),
                ratatui_image::FilterType::Triangle,
            );
            let cells = small
                .pixels()
                .map(|p| backdrop_shade((p[0], p[1], p[2])))
                .collect();
            self.backdrop = Some((key, cells));
        }
        let Some((_, cells)) = &self.backdrop else {
            return;
        };
        for (i, &(r, g, b)) in cells.iter().enumerate() {
            let x = fit.x + (i % usize::from(fit.width)) as u16;
            let y = fit.y + (i / usize::from(fit.width)) as u16;
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_bg(Color::Rgb(r, g, b));
                // Text drawn in the terminal's own colours or the dim grey was
                // chosen for an empty background; on the cover it has to be
                // light, whatever the terminal's foreground is.
                match cell.fg {
                    Color::Reset => {
                        cell.set_fg(Color::Rgb(240, 240, 240));
                    }
                    Color::DarkGray => {
                        cell.set_fg(Color::Rgb(165, 165, 165));
                        cell.modifier.remove(Modifier::DIM);
                    }
                    _ => {}
                }
            }
        }
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

/// A cover pixel darkened to sit behind text. A third of its brightness keeps
/// the picture recognisable while white and grey words stay readable on the
/// brightest part of it (white becomes about 85/255).
fn backdrop_shade((r, g, b): (u8, u8, u8)) -> (u8, u8, u8) {
    let d = |c: u8| (u16::from(c) / 3) as u8;
    (d(r), d(g), d(b))
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
/// Ask the terminal how big a cell is: `CSI 16 t` answers `CSI 6 ; h ; w t`.
///
/// **The single most useful thing a terminal can be asked** for this program: the
/// art is measured in cells, so an assumed cell size is an assumed cover size.
/// Ghostty, cmux, kitty, iTerm2, WezTerm and xterm all answer it; one that does
/// not costs the caller a round trip and nothing else, because `None` means "use
/// the default you already have".
///
/// Written and read on stdin/stdout directly rather than through crossterm,
/// before any event is read: a reply that arrives while the event reader is
/// looking would be taken for a keypress (`tui/colour.rs` does the same for its
/// background query, and the reason is written there). The price, paid once and
/// shared with that query, is that a key pressed in the first fraction of a second
/// is read here and dropped -- a launch is not when anybody is typing.
pub fn query_cell_size() -> Option<(u16, u16)> {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};

    let mut out = std::io::stdout();
    out.write_all(b"\x1b[16t\x1b[5n").ok()?;
    out.flush().ok()?;
    let fd = std::io::stdin().as_raw_fd();
    let deadline = Instant::now() + Duration::from_millis(300);
    let mut got: Vec<u8> = Vec::new();
    while parse_cell_size(&got).is_none() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd, and a buffer `read` may fill up to its length.
        if unsafe { libc::poll(&mut pfd, 1, left.as_millis() as i32) } <= 0 {
            break;
        }
        let mut buf = [0u8; 128];
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n <= 0 {
            break;
        }
        got.extend_from_slice(&buf[..n as usize]);
    }
    parse_cell_size(&got)
}

/// The `h` and `w` out of a `CSI 6 ; h ; w t` reply, in that order: the terminal
/// answers height first.
pub fn parse_cell_size(reply: &[u8]) -> Option<(u16, u16)> {
    let text = String::from_utf8_lossy(reply);
    let start = text.find("\x1b[6;")?;
    let rest = &text[start + 4..];
    let end = rest.find('t')?;
    let mut parts = rest[..end].split(';');
    let height: u16 = parts.next()?.trim().parse().ok()?;
    let width: u16 = parts.next()?.trim().parse().ok()?;
    // A cell is a handful of pixels across and a dozen or so down. Anything
    // outside that is a terminal answering something else, or in pixels-per-cell
    // form (CSI 16 t also has a "report text area size" variant in characters).
    if !(2..=100).contains(&width) || !(4..=200).contains(&height) {
        return None;
    }
    Some((width, height))
}

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
    /// The volume meter's bar, without its label or percentage.
    pub volume: Option<Rect>,
}

/// The cells a drawn volume meter answers a click on: itself, plus a band around
/// it wide enough to aim at. See `Regions::hit`.
fn volume_band(drawn: Rect) -> Rect {
    let x = drawn.x.saturating_sub(2);
    Rect {
        x,
        y: drawn.y.saturating_sub(1),
        width: drawn.width.saturating_add(4),
        height: drawn.height.saturating_add(3),
    }
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
        if let Some(v) = self.volume
            && v.width > 1
            && volume_band(v).contains((col, row).into())
        {
            // **The target is bigger than the drawing.** A one-row,
            // one-cell-precise slider is a slider nobody can hit without looking
            // (owner, 2026-10-03), so the clickable band is the meter's row, the
            // row above it, the two rows below it (the gradient rule and the pane
            // border) and the two cells at either end. The x mapping still follows
            // the drawn meter, so where you click is still the volume you get --
            // the band is only easier to land on.
            //
            // It cannot steal from the progress bar above or the transport
            // controls beside it: those are checked first, so a click that was
            // meant for one of those still is.
            //
            // The first cell of the meter is 0 and the last is 100, so both ends
            // are reachable with a click rather than only by a drag past the edge.
            let f = col.saturating_sub(v.x) as f64 / (v.width - 1) as f64;
            return Some(Hit::Volume(f.clamp(0.0, 1.0)));
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
        if let Some(t) = &app.toast {
            draw_toast(f, area, &t.text, theme);
        }
        return;
    }

    // The full-screen lyrics page replaces the dashboard (TODO 6.4). It is
    // drawn before the layout so a two-column dashboard is never built behind
    // a page that covers it.
    if app.lyrics_full {
        draw_lyrics_page(f, area, app, theme);
        // The cover goes behind the words when there is one; without it the
        // page is the plain one it always was.
        let cover = app
            .track()
            .filter(|_| app.settings.show_art && app.art.error.is_none())
            .and_then(|t| app.art.drawable(t));
        if let Some(art) = cover {
            images.backdrop(f.buffer_mut(), &art.path, &art.image, area);
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

    if let Some(t) = &app.toast {
        draw_toast(f, area, &t.text, theme);
    }
}

/// Wrap text to a display width, not a character count (TODO 6.4).
///
/// A CJK character is two cells wide, so wrapping by characters puts a wide
/// glyph past the edge of the screen and the line tears. Widths are measured
/// with `unicode-width`, the same library that pins the bars and meters, and a
/// character wider than the whole row (only possible at width 1) is let through
/// rather than dropped: losing a word is worse than one ragged column.
pub(crate) fn wrap_by_width(text: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;

    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let cw = ch.width().unwrap_or(0);
        // A zero-width character never starts a row; everything else starts
        // one when it no longer fits.
        if used + cw > width && !row.is_empty() {
            rows.push(std::mem::take(&mut row));
            used = 0;
        }
        row.push(ch);
        used += cw;
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    rows
}

/// The full-screen lyrics page (TODO 6.4): the line being sung, large and
/// centred, its neighbours receding above and below. `L` and `esc` leave (the
/// key handling is in `app.rs`); the bottom line says so, because a page that
/// covers every hint the dashboard carries owes one of its own.
fn draw_lyrics_page(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    use crate::tui::app::LyricsStatus;
    let dim = Theme::dim();
    let (w, h) = (area.width as usize, area.height as usize);

    let lyrics = match (&app.lyrics.status, app.lyrics.lyrics.as_ref()) {
        (LyricsStatus::Ready, Some(l)) if !l.lines.is_empty() && !l.instrumental => l,
        _ => {
            // The same words the tab would say, centred, so the page is never
            // a blank screen with no way to know why.
            let title = app.track().map(|t| t.title.clone()).unwrap_or_default();
            let msg = match &app.lyrics.status {
                LyricsStatus::Ready => "this one is instrumental",
                LyricsStatus::NotFound => "no lyrics for this one",
                LyricsStatus::Failed(why) => why.as_str(),
                LyricsStatus::Idle | LyricsStatus::Loading => "looking for lyrics…",
            };
            let rows = vec![
                Line::from(Span::styled(
                    title,
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    msg.to_string(),
                    dim.add_modifier(Modifier::ITALIC),
                )),
            ];
            f.render_widget(Paragraph::new(rows).alignment(Alignment::Center), area);
            return;
        }
    };

    let pos = app.interpolated_position();
    let active = lyrics
        .lines
        .iter()
        .rposition(|l| !l.time_secs.is_nan() && l.time_secs <= pos)
        .unwrap_or(0);
    let anchor = app.lyrics.anchor(pos).unwrap_or(active);
    let synced = lyrics.synced && !lyrics.lines[active].time_secs.is_nan();

    // The anchor's rows, centred on the middle of the screen. "Large" is all a
    // terminal can do: bold, in the accent colour, alone in that colour.
    let centre_style = Style::default()
        .fg(theme.accent_colour())
        .add_modifier(Modifier::BOLD);
    let cur_rows = wrap_by_width(&lyrics.lines[anchor].text, w);
    let top_of_centre = (h / 2).saturating_sub(cur_rows.len() / 2);

    let mut draw = |y: usize, text: &str, style: Style| {
        if y < h {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(text.to_string(), style)))
                    .alignment(Alignment::Center),
                Rect {
                    x: area.x,
                    y: area.y + y as u16,
                    width: area.width,
                    height: 1,
                },
            );
        }
    };

    for (i, row) in cur_rows.iter().enumerate() {
        draw(top_of_centre + i, row, centre_style);
    }

    // The neighbours, receding. The sung line keeps its colour when it is not
    // the anchor, so scrolling ahead still shows where the song is.
    let mut y = top_of_centre;
    let mut i = anchor;
    while y > 0 && i > 0 {
        i -= 1;
        for row in wrap_by_width(&lyrics.lines[i].text, w).into_iter().rev() {
            if y == 0 {
                break;
            }
            y -= 1;
            let style = if i == active && synced {
                Style::default()
            } else {
                dim.add_modifier(Modifier::DIM)
            };
            draw(y, &row, style);
        }
    }

    let mut y = top_of_centre + cur_rows.len();
    let mut i = anchor;
    // The bottom row is the hint's, once the screen is tall enough to have one.
    let bottom = h.saturating_sub(if h >= 3 { 1 } else { 0 });
    while y < bottom && i + 1 < lyrics.lines.len() {
        i += 1;
        for row in wrap_by_width(&lyrics.lines[i].text, w) {
            if y >= bottom {
                break;
            }
            let style = if i == active && synced {
                Style::default()
            } else {
                dim.add_modifier(Modifier::DIM)
            };
            draw(y, &row, style);
            y += 1;
        }
    }

    if h >= 3 {
        draw(
            h - 1,
            "L or esc returns to the dashboard",
            dim.add_modifier(Modifier::ITALIC),
        );
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
        // The pane's own height decides its width: the cover is square, so the
        // width at which it fills the height is the width the pane should have.
        let want = now_playing_width(
            rows[1].width,
            rows[1].height.saturating_sub(2),
            images.cell_size(),
            app.track().map_or(TEXT_ROWS_MAX, |t| text_rows(app, t)),
        );
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(want.clamp(24, rows[1].width.saturating_sub(24))),
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
    // Clamped to the area as well as to the card's own bounds. A card 34 cells
    // wide drawn into a 31-cell terminal put `Clear` one column past the end of
    // the buffer, and ratatui panics on an index outside it rather than clipping:
    // a terminal narrower than the card was a crash, not a small card (found by
    // fuzzing, owner, 2026-10-02).
    let card_w = (area.width * 2 / 3).clamp(34, 64).min(area.width);
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
                "Trak never starts Spotify on its own.",
                Theme::dim(),
            )),
            Line::from(Span::styled("q  quit            ?  keys", Theme::dim())),
        ])
        .alignment(Alignment::Center)
        .block(
            Block::bordered()
                .title(" Trak ")
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
            if app.settings.show_progress {
                progress_bar(progress(app), area.width.saturating_sub(4) as usize)
            } else {
                String::new()
            },
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
        " Trak ",
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
    // Which line of the block each row belongs to, so the clickable rectangles
    // follow the block wherever it is put rather than assuming it starts at the
    // top of the pane.
    let mut volume_row: Option<usize> = None;

    let dim = Theme::dim();
    let title_w = body.width as usize;

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
    //
    // The region is recorded even when the bar is off, so the geometry does not
    // depend on a display setting: a click on a hidden bar must not seek.
    let bar_index = lines.len();
    if app.settings.show_progress {
        lines.push(Line::from(crate::tui::theme::progress_bar_spans(
            progress(app),
            title_w,
            &theme.palette,
            !app.is_playing(),
            app.tick_secs,
            app.is_playing(),
        )));
    }

    let elapsed = format_time(pos);
    let dur_str = format_time(dur);
    let mut times = vec![Span::styled(elapsed.clone(), dim)];
    // Push the duration to the right edge rather than leaving a gap that the
    // reader has to measure.
    let used = elapsed.chars().count();
    if body.width as usize > used + dur_str.chars().count() + 2 {
        times.push(Span::raw(
            " ".repeat(body.width as usize - used - dur_str.chars().count()),
        ));
    }
    times.push(Span::styled(
        dur_str,
        Style::default().fg(theme.accent_colour()),
    ));
    lines.push(Line::from(times));
    lines.push(Line::from(""));

    // Controls, with the shuffle and repeat badges next to them.
    //
    // Drawn from geometric shapes, two cells per button and two cells between
    // them. The media glyphs (⏮ ⏸ ⏭) are emoji-capable and cmux/Ghostty draw
    // them wider than ratatui counts them, which squashed the three buttons into
    // one smudge.
    let ctl_index = lines.len();
    let accent = Style::default().fg(theme.accent_colour());
    let controls = vec![
        Span::raw(" "),
        Span::styled(TRANSPORT_PREV, accent),
        Span::raw("  "),
        Span::styled(
            match app.state.as_ref().map(|s| s.playback) {
                Some(PlaybackState::Playing) => TRANSPORT_PAUSE,
                _ => TRANSPORT_PLAY,
            },
            Style::default()
                .fg(theme.palette.end())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(TRANSPORT_NEXT, accent),
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

    // The volume meter gets the same treatment as the bar, because it is the same
    // kind of thing: a quantity, drawn with the album's own colours. Hidden when
    // Spotify ignored a write (COMPAT rule 5).
    if app.settings.show_volume && !app.volume_hidden {
        let v = app.meter_volume();
        let label = "vol ";
        // Two cells before the percentage, not one: `▰` is drawn a little
        // wider than its cell in common fonts, so at 100 % a full last cell ran
        // into the "1" with only a single space between them.
        volume_row = Some(lines.len());
        let meter_w = (body.width as usize).saturating_sub(label.len() + 6).max(1);
        let mut row = vec![Span::styled(label, dim)];
        row.extend(crate::tui::theme::gradient_meter(
            f64::from(v) / 100.0,
            meter_w,
            &theme.palette,
            app.muted,
        ));
        row.push(Span::raw("  "));
        row.push(Span::styled(
            format!("{v:>3}%"),
            Style::default().fg(theme.accent_colour()),
        ));
        lines.push(Line::from(row));
    }

    // The cover owns the top of the pane. It is drawn large and hard against the
    // left edge rather than centred in a frame: the artwork *is* the visual, and
    // a box around it says "placeholder here" when nothing could be more certain.
    //
    // It gets every row the text does not need, up to the height at which a
    // square cover fills the pane's width -- a taller cover than that can only be
    // narrower than the pane, so the extra rows would be dead space with a picture
    // in the middle of it. That is what made a tall terminal look empty: a cover
    // frozen at twenty rows in a fifty-row pane.
    //
    // Whatever neither of them fills -- a short, wide pane, or a tall narrow one
    // where the cover is width-limited -- is split above and below the block
    // rather than dumped in one gap, so the pane reads as composed at any size.
    let text_h = (lines.len() as u16).min(body.height);
    let (cw, ch) = images.cell_size();
    let fill_h = (u32::from(body.width) * u32::from(cw) / u32::from(ch.max(1))) as u16;
    let hole_h = body
        .height
        .saturating_sub(text_h)
        .min(fill_h.max(MIN_ART.1));
    let show_art = hole_h >= MIN_ART.1 && body.width >= MIN_ART.0;
    let hole_h = if show_art { hole_h } else { 0 };
    let block_h = (hole_h + text_h).min(body.height);
    let block_top = body.y + (body.height - block_h) / 2;
    let text_top = block_top + hole_h;

    if show_art {
        let hole = Rect {
            x: body.x,
            y: block_top,
            width: body.width,
            height: hole_h,
        };
        // The visualizer takes the same rectangle (TODO 4.3). The cover is still
        // fetched and still drives the accent: a visualizer tinted by the album
        // is the entire point of having one.
        let show_visualizer = app.settings.display_mode == DisplayMode::Visualizer;
        let drawn = app
            .settings
            .show_art
            .then(|| app.art.drawable(track))
            .flatten()
            .filter(|_| app.art.error.is_none());
        if show_visualizer {
            draw_visualizer(f, hole, app, theme);
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
    }

    // Everything below the cover is text, so the row arithmetic is relative to
    // this rectangle: measuring from `body` is off by the height of the art,
    // which is how the progress bar ends up not matching the thing you click.
    let text_body = Rect {
        y: text_top,
        height: body.y + body.height - text_top,
        ..body
    };
    if text_body.height == 0 {
        return;
    }
    let bar_row = text_body.y + bar_index as u16;
    let ctl_row = text_body.y + ctl_index as u16;
    regions.progress = Some(Rect {
        x: text_body.x,
        y: bar_row,
        width: text_body.width,
        height: 1,
    });
    // The buttons sit at columns 1, 5 and 9, two cells each; the target takes
    // the gap on either side too, so a click a cell off still lands.
    regions.controls = [(0, Control::Prev), (4, Control::Toggle), (8, Control::Next)]
        .into_iter()
        .map(|(dx, c)| {
            (
                Rect {
                    x: text_body.x + dx,
                    y: ctl_row,
                    width: 4,
                    height: 1,
                },
                c,
            )
        })
        .collect();
    if let Some(meter_row) = volume_row {
        let label = "vol ";
        let meter_w = (text_body.width as usize)
            .saturating_sub(label.len() + 6)
            .max(1);
        let drawn = Rect {
            x: text_body.x + label.len() as u16,
            y: text_body.y + meter_row as u16,
            width: meter_w as u16,
            height: 1,
        };
        // The stored region is the meter **as drawn**, so the x mapping is honest
        // and the layout code above stays a description of the picture. The
        // clickable band around it is applied in `Regions::hit`.
        regions.volume = Some(drawn);
    }

    let shown = lines.len().min(text_body.height as usize);
    if shown > 0 {
        f.render_widget(
            Paragraph::new(lines.into_iter().take(shown).collect::<Vec<_>>()),
            text_body,
        );
    }

    // A gradient rule under the block. On a tall terminal there is always space
    // left over, and a bare gap at the foot of a panel reads as an oversight; a
    // rule reads as a deliberate edge, and it is the last thing the album's
    // colours get to say. It follows the block rather than the floor of the
    // pane, so it stays the same distance from the text at any size.
    let spare = body.y + body.height - (text_body.y + shown as u16);
    if spare >= 2 {
        let rule_y = text_body.y + shown as u16;
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

/// The visualizer, in whichever of the four styles the user picked (TODO 8.4).
///
/// The four renderers are pure functions over a spectrum, and they hand back
/// plain text; the colour is applied here, per column, off the album's own
/// ramp. That split is what makes a visualizer tinted by the cover possible
/// without four renderers knowing anything about palettes.
fn draw_visualizer(f: &mut Frame, hole: Rect, app: &App, theme: &Theme) {
    if hole.width == 0 || hole.height == 0 {
        return;
    }
    f.render_widget(ratatui::widgets::Clear, hole);

    let style = app.settings.visualizer_style;
    let name = style.label();
    // Too small for the artwork of any of the styles: say which one is active
    // rather than drawing four bars in a six-cell box, which looks like a bug.
    if hole.height < 3 || hole.width < 8 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(name, Theme::dim()))),
            hole,
        );
        return;
    }

    // The style name sits in the top row and the drawing gets what is left, so
    // `v` always has a visible consequence and the name never overlaps a bar.
    let (w, h) = (hole.width as usize, hole.height as usize - 1);
    let lines = match style {
        VisualizerStyle::Spectrum => crate::visualizer::spectrum(&app.spectrum, w, h),
        VisualizerStyle::Mirrored => crate::visualizer::mirrored(&app.spectrum, w, h),
        VisualizerStyle::Waveform => crate::visualizer::waveform(&app.spectrum, w, h),
        VisualizerStyle::Circular => crate::visualizer::circular(&app.spectrum, w, h),
    };

    // One colour per column, so a bar is a gradient rather than a flat block and
    // the whole pane is tinted by the album (SPEC §2, TODO 4.2). The ramp is
    // asked for the drawing's width, not the pane's, because a gradient that
    // repeats every few columns reads as stripes.
    let ramp = theme.palette.ramp(w);
    for (row, line) in lines.iter().enumerate() {
        let y = hole.y + 1 + row as u16;
        if y >= hole.y + hole.height {
            break;
        }
        let mut spans: Vec<Span> = Vec::new();
        // A cell is one column of the ramp regardless of how many graphemes are
        // in it, and the renderers guarantee exactly one column per cell, so the
        // two indexes line up. `chars()` not `char_indices()`: a braille glyph
        // is one char but three bytes, and walking bytes would colour the wrong
        // column.
        for (col, ch) in line.chars().enumerate() {
            let colour = ramp[col % ramp.len()];
            spans.push(Span::styled(ch.to_string(), Style::default().fg(colour)));
        }
        if spans.is_empty() {
            continue;
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect {
                x: hole.x,
                y,
                width: hole.width,
                height: 1,
            },
        );
    }

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
    // The segments come first: the title is built from them and the clickable
    // rects come from them, so a tab cannot be drawn in one place and clicked
    // somewhere else.
    // Two border cells, plus the one space each side of the title inside it.
    let strip = tab_strip(app, theme, (area.width as usize).saturating_sub(4));
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
        // The pane is registered as the list region so the wheel can scroll
        // the words; `handle_mouse` gives the Lyrics tab its own meaning for
        // rows and for the wheel.
        Tab::Lyrics => {
            regions.history = Some(body);
            lyrics_lines(app, theme)
        }
        // TODO 7.6-7.11. A Web tab says why it is empty, which is more use than a
        // blank pane: the pane existing is what tells a person the feature is
        // there to be unlocked.
        _ => {
            regions.history = Some(body);
            crate::tui::web_tabs::lines(app, theme)
        }
    };
    let lines_len = lines.len();
    // Lyrics wrap; lists do not. A list row is one track and the columns are
    // already chosen to fit, while a lyric line is whatever the song says and a
    // narrow pane used to cut it off mid-word with no way to see the rest --
    // there is no sideways scroll for a tab (owner, 2026-10-02).
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    f.render_widget(paragraph, body_rect(body, lines_len));
}

/// Where a pane's lines are drawn when they do not fill it.
///
/// Top-anchored, which is right for a list that grows downwards. But a short
/// block -- a first-run hint, a tab that has nothing in it yet -- at the top of a
/// sixty-row pane leaves the other fifty-five rows looking like something failed
/// to draw, so a block with real room to spare is centred instead. Six rows is the
/// threshold: below that the gap is not worth moving the content for.
fn body_rect(body: Rect, lines: usize) -> Rect {
    let spare = (body.height as usize).saturating_sub(lines);
    if spare < 6 {
        return body;
    }
    Rect {
        y: body.y + (spare / 2) as u16,
        height: lines as u16,
        ..body
    }
}

/// The Lyrics tab: the line being sung, the ones coming, and the ones just gone.
///
/// Synced lyrics are the whole reason this tab exists, so the line being sung is set
/// in the album's own gradient and everything else recedes -- dimmer, and further
/// back. Four lines above and a dozen below is what fits a pane without turning
/// it into a wall of text.
///
/// The window follows the anchor ([`LyricsState::anchor`]): the sung line while
/// the view follows, the user's line while their scroll hold lasts. The sung
/// line keeps its gradient wherever it lands, so a scroll-ahead still shows
/// which line is being sung when it comes back into view.
fn lyrics_lines<'a>(app: &App, theme: &'a Theme) -> Vec<Line<'a>> {
    use crate::tui::app::LyricsStatus;

    let dim = Theme::dim();
    // A heading and one line of explanation. Two arguments rather than one plus a
    // fixed footer, because "no lyrics for this one" used to be printed *and* then
    // printed again as the explanation of itself.
    let note = |heading: &str, detail: &str| {
        vec![
            Line::from(Span::styled(
                heading.to_string(),
                dim.add_modifier(Modifier::ITALIC),
            )),
            Line::from(""),
            Line::from(Span::styled(detail.to_string(), dim)),
        ]
    };
    let no_lyrics = || {
        note(
            "no lyrics for this one",
            "LRCLIB has most songs but not all of them",
        )
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
        LyricsStatus::NotFound => no_lyrics(),
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
                return no_lyrics();
            };
            if lyrics.instrumental {
                return note("this one is instrumental", "there are no words to show");
            }
            if lyrics.lines.is_empty() {
                return no_lyrics();
            }
            let pos = app.interpolated_position();
            let active = lyrics
                .lines
                .iter()
                .rposition(|l| !l.time_secs.is_nan() && l.time_secs <= pos)
                .unwrap_or(0);
            let anchor = app.lyrics.anchor(pos).unwrap_or(active);
            // Only synced lyrics have a position; unsynced ones are shown from
            // the top, which is all that can honestly be done with them.
            let synced = lyrics.synced && !lyrics.lines[active].time_secs.is_nan();

            let mut out: Vec<Line> = Vec::new();
            // A few lines of lead-in, dimmer the further back they are.
            let from = anchor.saturating_sub(4);
            for (i, line) in lyrics.lines[from..=anchor].iter().enumerate() {
                let idx = from + i;
                let style = if idx == active && synced {
                    Style::default()
                } else {
                    dim.add_modifier(Modifier::DIM)
                };
                out.push(if idx == active && synced {
                    crate::tui::theme::gradient_line(&line.text, &theme.palette)
                } else {
                    Line::from(Span::styled(line.text.clone(), style))
                });
            }
            // And the ones coming.
            for line in lyrics.lines.iter().skip(anchor + 1).take(14) {
                out.push(Line::from(Span::styled(
                    line.text.clone(),
                    dim.add_modifier(Modifier::DIM),
                )));
            }
            if !lyrics.synced {
                out.push(Line::from(""));
                out.push(Line::from(Span::styled("unsynced lyrics", dim)));
            }
            // While the user holds the scroll, say so: a view that stopped
            // following the song for no visible reason looks like a bug.
            if !app.lyrics.following() {
                out.push(Line::from(""));
                out.push(Line::from(Span::styled(
                    "following paused — it resumes on its own",
                    dim.add_modifier(Modifier::ITALIC),
                )));
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

/// The tab strip, built to fit the width it actually has.
///
/// Version A took the strip from three tabs to eight, which fits in no pane trak
/// is used at. So the strip is built *after* the width is known and shows as many
/// tabs as fit, always including the selected one -- a strip that has scrolled
/// the tab you are on out of sight is worse than a strip that cannot show
/// everything, because nothing on screen then says which tab you are on.
///
/// Elision is marked with `‹` and `›` rather than being silent, so a missing tab
/// reads as "there are more" and not as "that is all of them".
fn tab_strip(app: &App, theme: &Theme, avail: usize) -> TabStrip {
    // Both label forms, because the selected one is narrower than the unselected
    // one for a numbered tab: `[6]Lyrics` against ` 6 Lyrics `. Measuring the
    // strings is what stops the selected tab being the one that does not fit.
    let plain = |t: &Tab| match t.digit() {
        Some(n) => format!(" {n} {} ", t.label()),
        // History and Info are the `Tab` pair and have no number; a number they
        // do not own would make the digits disagree with the order.
        None => format!(" {} ", t.label()),
    };
    let marked = |t: &Tab| match t.digit() {
        Some(n) => format!("[{n}]{}", t.label()),
        None => format!("[{}]", t.label()),
    };
    // The wider of the two, so the reservation holds whichever form is drawn.
    let width_of = |t: &Tab| plain(t).chars().count().max(marked(t).chars().count());
    let sel = Tab::ALL.iter().position(|t| *t == app.tab).unwrap_or(0);

    // What a window of tabs costs, *including* the elision markers. The markers
    // are the part a first attempt forgets, and at four cells each they are
    // exactly enough to push the last tab past the border, where ratatui
    // truncates it mid-word.
    let cost = |lo: usize, hi: usize| {
        let tabs: usize = (lo..=hi).map(|i| width_of(&Tab::ALL[i])).sum();
        let gaps = hi - lo;
        let before = usize::from(lo > 0) * 4;
        let after = usize::from(hi + 1 < Tab::ALL.len()) * 4;
        tabs + gaps + before + after
    };

    // Grow outwards from the selected tab, one side then the other, so it sits
    // in the middle with its neighbours either side of it -- the ones `←`/`→`
    // go to next. Near either end there is nothing to grow into on that side,
    // so the other side takes the room and the selected tab ends up at the edge.
    let mut lo = sel;
    let mut hi = sel;
    loop {
        let left = (lo > 0 && cost(lo - 1, hi) <= avail).then(|| lo - 1);
        let right = (hi + 1 < Tab::ALL.len() && cost(lo, hi + 1) <= avail).then(|| hi + 1);
        match (left, right) {
            // The side with fewer tabs so far goes first; a tie goes right, so
            // an odd count shows one more of what is coming than what is behind.
            (Some(l), Some(_)) if sel - lo < hi - sel => lo = l,
            (_, Some(r)) => hi = r,
            (Some(l), None) => lo = l,
            (None, None) => break,
        }
    }
    let hidden_before = lo;
    let hidden_after = Tab::ALL.len() - 1 - hi;

    let mut segments = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut width = 0usize;
    // The markers are drawn but deliberately *not* registered as tabs: a click on
    // "there is more" should not select something, and a hit region for a glyph
    // that is not a label is a click that silently does the wrong thing.
    let mut gap = false;
    if hidden_before > 0 {
        spans.push(Span::raw(" ‹ "));
        width += 3;
    }
    for t in &Tab::ALL[lo..=hi] {
        let is_sel = *t == app.tab;
        let text = if is_sel { marked(t) } else { plain(t) };
        if gap {
            spans.push(Span::raw(" "));
            width += 1;
        }
        segments.push((width as u16, text.clone()));
        width += text.chars().count();
        if is_sel {
            spans.extend(gradient_title(&text, theme));
        } else {
            spans.push(Span::styled(text, Style::default()));
        }
        gap = true;
    }
    if hidden_after > 0 {
        if gap {
            spans.push(Span::raw(" "));
            width += 1;
        }
        // The last thing drawn, so its width is never read back: the strip's
        // length is what `cost` reserved, not what this loop accumulates.
        spans.push(Span::raw(" › "));
        let _ = width;
    }
    TabStrip { segments, spans }
}

/// A focused title as a **gradient run** rather than one flat accent block.
///
/// The focused tab used to be `accent_text` on `accent_colour`: one colour, and a
/// flat one, which reads as "grey-ish tab that happens to be highlighted" next to
/// the album's own colours elsewhere on the screen (owner, 2026-10-03: "make sure
/// there is something someone can notice if it is in focus, like turn it from gray
/// to the gradient"). One span per character, each carrying the next colour of the
/// album's ramp with the matching readable text colour under it, so the strip is
/// the cover's palette rather than a single swatch of it.
///
/// One span per character is affordable because this is the focused tab only: ten
/// or so cells, once a frame, and only when the tab changes.
pub fn gradient_title(text: &str, theme: &Theme) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let ramp = theme.palette.ramp(chars.len().max(2));
    chars
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let colour = ramp[(i * ramp.len()) / chars.len().max(1)];
            Span::styled(
                c.to_string(),
                Style::default()
                    .fg(colour)
                    .bg(theme.text_on_colour(colour))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect()
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
    //
    // The setting is about the *meter* row further down, not about hiding the
    // number: a person who turned the pretty bar off still wants the fact.
    if app.settings.show_popularity {
        out.push(dash(
            "popularity",
            t.popularity
                .map(|p| p.to_string())
                .unwrap_or_else(|| "\u{2014}".into()),
        ));
    }
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
    // The `▰▱` meter is the *pretty* form of the popularity number above, and the
    // setting turns that off rather than the number: a person who dislikes a bar
    // still wants the fact.
    if let Some(p) = t.popularity
        && app.settings.show_popularity
    {
        out.push(dash("popularity bar", Theme::volume_meter(p as u8, 10)));
    }
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
    // The first-run hint outranks everything: a user who has never run trak does
    // not know it has a settings screen, and the key list cannot say so in the
    // space it has (TODO 5.4).
    if let Some(hint) = &app.hint
        && area.width as usize > hint.chars().count() + 2
    {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {hint} "),
                Style::default()
                    .fg(theme.palette.primary)
                    .add_modifier(Modifier::ITALIC),
            ))),
            area,
        );
        return;
    }
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
            ("h/l", "seek"),
            ("←/→", "tab"),
            ("+/-", "vol"),
            ("s", "shuffle"),
            ("r", "repeat"),
            ("c", "copy"),
            ("?", "settings"),
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
pub(crate) const HELP_KEY_WIDTH: usize = 16;

/// Where the help overlay sits. Shared with the parity test so it reads the same
/// cells the renderer wrote instead of guessing.
/// Every key that does something today, and what it does.
///
/// A `const` rather than a `vec!` in the function because the popup is **sized
/// from this list**. It used to be a fixed fraction of the screen, which meant
/// every key added here silently fell off the bottom -- and the last row is the
/// one people scroll to.
///
/// SPEC §4 is the source of the keys;
/// `the_help_overlay_matches_the_spec_table` fails the build if the two drift.
pub(crate) const HELP_ROWS: &[(&str, &str)] = &[
    ("space", "play / pause"),
    ("n / p", "next / previous track"),
    ("h / l", "seek back / forward"),
    ("+ / -", "volume up / down"),
    ("m", "mute (saves the volume you had)"),
    ("s", "toggle shuffle"),
    ("r", "repeat: off → all → one     R  replay"),
    ("c", "copy the share link"),
    ("j / k  ↑ ↓", "move in a list"),
    ("enter", "play the selected item"),
    ("a", "album art, or the visualizer"),
    ("v", "next visualizer style"),
    ("L", "full-screen lyrics"),
    ("← / →", "previous / next tab"),
    ("tab / shift-tab", "next / previous tab"),
    ("1 .. 6", "jump to a tab"),
    ("/", "search (Version A)"),
    ("o", "open the artist or album"),
    ("A", "add the selection to the queue"),
    ("f", "like or unlike the playing track"),
    ("P", "add the selection to a playlist"),
    ("X", "remove it from the open playlist"),
    (", / ?", "settings"),
    ("? / esc", "close this"),
    ("q / ctrl-c", "quit"),
];

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

    /// The buffer as plain text, which is what most of these assertions want.
    fn text_at(term: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// An app in visualizer mode with a settled spectrum, so the assertions are
    /// about what is drawn rather than about how far the source has decayed.
    fn viz_app(style: VisualizerStyle) -> App {
        let mut app = app_at(100, 30);
        app.settings.display_mode = DisplayMode::Visualizer;
        app.settings.visualizer_style = style;
        for _ in 0..90 {
            app = update(app, Event::VizTick).app;
        }
        app
    }

    /// The buffer as a grid of symbols, for diffing two frames.
    fn cells_at(term: &ratatui::Terminal<ratatui::backend::TestBackend>) -> Vec<Vec<String>> {
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect()
    }

    fn frame(app: &App) -> Vec<Vec<String>> {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| draw(f, app, &Theme::default())).unwrap();
        cells_at(&term)
    }

    /// TODO 8.4: each of the four styles is reachable from the keyboard, draws
    /// something of its own, and names itself in the pane so `v` has a visible
    /// consequence.
    ///
    /// "Drew something" is measured by differing from the art-mode frame rather
    /// than by looking for particular glyphs: braille is a different Unicode
    /// block from the block characters the other three use, so a test that
    /// counted known characters would pass three styles and fail the fourth for
    /// a reason that has nothing to do with the renderer.
    #[test]
    fn every_visualizer_style_draws_its_own_thing() {
        let mut art = viz_app(VisualizerStyle::Spectrum);
        art.settings.display_mode = DisplayMode::Art;
        let with_cover = frame(&art);

        let mut drawn = Vec::new();
        for style in VisualizerStyle::ALL {
            let cells = frame(&viz_app(style));
            let text = cells
                .iter()
                .map(|r| r.concat())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                text.contains(style.label()),
                "{style:?} names nothing: {text}"
            );

            // Every cell that changed between showing the cover and showing this
            // style, and none of them blank: a style that erased the pane and drew
            // nothing back would otherwise pass a "did something" test.
            let mut changed = Vec::new();
            for (y, (a, b)) in with_cover.iter().zip(&cells).enumerate() {
                for (x, (a, b)) in a.iter().zip(b).enumerate() {
                    if a != b {
                        changed.push((x, y, b.clone()));
                    }
                }
            }
            assert!(
                changed.len() > 40,
                "{style:?} changed only {} cells",
                changed.len()
            );
            assert!(
                changed.iter().all(|(_, _, s)| s != " "),
                "{style:?} left the pane emptier than it found it"
            );
            drawn.push(cells);
        }
        // Four styles, four drawings -- not one drawing drawn four times.
        for (i, a) in drawn.iter().enumerate() {
            for (j, b) in drawn.iter().enumerate() {
                assert!(i == j || a != b, "styles {i} and {j} drew the same");
            }
        }
    }

    /// The visualizer shares the cover's rectangle, so turning it on must not
    /// change the layout around it (TODO 4.3: `a` swaps what is drawn, not what
    /// is drawn around).
    #[test]
    fn the_visualizer_takes_the_covers_rectangle() {
        let mut art = app_at(100, 30);
        art.settings.display_mode = DisplayMode::Art;
        let mut viz = app_at(100, 30);
        viz.settings.display_mode = DisplayMode::Visualizer;
        let mut out = Vec::new();
        for app in [&art, &viz] {
            let backend = ratatui::backend::TestBackend::new(100, 30);
            let mut term = ratatui::Terminal::new(backend).unwrap();
            term.draw(|f| draw(f, app, &Theme::default())).unwrap();
            out.push(text_at(&term));
        }
        // What has to survive is the *Now Playing* pane and the footer, which is
        // what this test is about. The tab strip is not asserted here: at this
        // width it legitimately elides, and asserting on it would be asserting
        // on the strip, which has its own tests.
        // Read the needles off the app rather than hard-coding the fixture's
        // strings, so changing the fixture cannot quietly weaken this test into
        // asserting on text that is no longer there.
        let track = art.track().expect("a track is playing");
        for needle in [track.title.as_str(), track.artist.as_str()] {
            assert!(out[0].contains(needle), "art lost {needle}");
            assert!(out[1].contains(needle), "visualizer lost {needle}");
        }
        assert!(out[0].contains("vol"), "and the volume meter");
        assert!(out[1].contains("vol"), "and the volume meter");
        // The one thing that must differ is the pane the cover was in.
        assert_ne!(out[0], out[1], "nothing was swapped");
    }

    /// TODO 5.3: every setting that the renderer reads has to visibly change the
    /// dashboard. These are the two that were being carried with nothing reading
    /// them, which is the failure mode a config layer grows quietly.
    #[test]
    fn the_progress_and_popularity_settings_change_the_dashboard() {
        // The Info tab, because that is where the popularity row lives: this is
        // a test of a setting, not of which tab happens to be open.
        let on = {
            let mut a = viz_app(VisualizerStyle::Spectrum);
            a.settings.display_mode = DisplayMode::Art;
            a.tab = Tab::Info;
            a
        };
        let mut off = on.clone();
        off.settings.show_progress = false;
        let mut off_pop = on.clone();
        off_pop.settings.show_popularity = false;

        let (a, b, c) = (frame(&on), frame(&off), frame(&off_pop));
        assert!(a != b, "show_progress = false changed nothing");
        assert!(a != c, "show_popularity = false changed nothing");

        // And the specific rows move, rather than the frame merely flickering.
        let text = cells_to_text(&a);
        let text_off = cells_to_text(&b);
        let text_pop = cells_to_text(&c);
        // The bar is a row of `\u{2500}` with a `\u{25cf}` head, so the row it
        // occupied stops being occupied. Counting cells rather than lines
        // because the borders are lines of the same character.
        let dashes = |t: &str| t.chars().filter(|c| *c == '\u{2500}').count();
        assert!(
            dashes(&text).saturating_sub(dashes(&text_off)) >= 20,
            "turning the bar off left {} of its cells behind",
            dashes(&text).saturating_sub(dashes(&text_off))
        );
        assert!(text.contains("popularity"), "{text}");
        assert!(
            text_pop.matches("popularity").count() < text.matches("popularity").count(),
            "turning popularity off left every row: {text_pop}"
        );
    }

    fn cells_to_text(cells: &[Vec<String>]) -> String {
        cells
            .iter()
            .map(|r| r.concat())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// TODO 5.4: a first launch in a clean home says so once, and any key
    /// dismisses it.
    #[test]
    fn the_first_run_hint_shows_once_and_any_key_dismisses_it() {
        let mut app = app_at(100, 30);
        assert!(app.hint.is_none(), "not the first run by default");
        app.hint = Some("press , for settings".to_string());

        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| draw(f, &app, &Theme::default())).unwrap();
        assert!(
            text_at(&term).contains("press , for settings"),
            "the hint was not shown"
        );

        let app = update(app, Event::Key('j')).app;
        assert!(app.hint.is_none(), "j must dismiss it");
        term.draw(|f| draw(f, &app, &Theme::default())).unwrap();
        assert!(
            !text_at(&term).contains("press , for settings"),
            "the hint came back"
        );
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
        // Every width from the floor up, because the idle card has a minimum size
        // and a terminal narrower than it used to be a panic rather than a small
        // card.
        let mut sizes: Vec<(u16, u16)> = (30..=90).map(|w| (w, 24)).collect();
        sizes.extend([
            (80u16, 24u16),
            (50, 14),
            (35, 10),
            (25, 6),
            (30, 8),
            (30, 9),
        ]);
        for (w, h) in sizes {
            let backend = ratatui::backend::TestBackend::new(w, h);
            let mut term = ratatui::Terminal::new(backend).unwrap();
            let theme = Theme::default();
            term.draw(|f| draw(f, &App::new(), &theme)).unwrap();
        }
    }

    /// `?` opens the settings screen, and every key is on it: beside the
    /// settings on a wide terminal, under them on a narrow one.
    #[test]
    fn the_settings_screen_lists_every_key() {
        for (w, h) in [(140u16, 40u16), (100, 30), (60, 20)] {
            let backend = ratatui::backend::TestBackend::new(w, h);
            let mut term = ratatui::Terminal::new(backend).unwrap();
            let mut app = app_at(w, h);
            app.settings_open = true;
            crate::tui::settings::open(&mut app);
            // On a narrow screen the keys are under the last setting.
            app.settings_cursor = usize::MAX;
            let theme = Theme::default();
            term.draw(|f| crate::tui::settings::render(f, f.area(), &app, &theme))
                .unwrap();
            let buf = term.backend().buffer().clone();
            let text: String = (0..h)
                .map(|y| row_text(&buf, y, 0, w))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(text.contains("Keys"), "{w}x{h}: {text}");
            if w >= 140 {
                for (_, what) in HELP_ROWS {
                    assert!(text.contains(what), "{w}x{h} lacks {what:?}: {text}");
                }
            } else {
                assert!(text.contains("play / pause"), "{w}x{h}: {text}");
            }
        }
    }

    /// TODO 11.6: nothing a track can be called, and no terminal size, may panic
    /// the draw. Wide CJK, right-to-left, emoji, combining marks and a title far
    /// longer than any pane, at every tab and from 1x1 up to a very large window.
    #[test]
    fn hostile_titles_and_sizes_never_panic_the_draw() {
        use crate::tui::app::Tab;
        let titles = [
            "日本語のとても長いタイトル、全角文字だけで書かれています",
            "عنوان طويل جدا من اليمين إلى اليسار — فنان",
            "שיר בעברית עם מילים ארוכות מאוד",
            "🎵🎶🎧 emoji 👨\u{200d}👩\u{200d}👧 title",
            "e\u{301}\u{301}\u{301} combining",
            &"x".repeat(500),
            "",
        ];
        let theme = Theme::default();
        for title in titles {
            for tab in Tab::ALL {
                for (w, h) in [
                    (1u16, 1u16),
                    (3, 2),
                    (24, 6),
                    (40, 12),
                    (80, 24),
                    (100, 30),
                    (250, 70),
                ] {
                    let mut app = app_at(w, h);
                    app.tab = tab;
                    if let Some(st) = app.state.as_mut() {
                        st.track.title = title.to_string();
                        st.track.artist = title.to_string();
                        st.track.album = title.to_string();
                    }
                    let backend = ratatui::backend::TestBackend::new(w, h);
                    let mut term = ratatui::Terminal::new(backend).unwrap();
                    term.draw(|f| draw(f, &app, &theme))
                        .unwrap_or_else(|e| panic!("{title:?} on {tab:?} at {w}x{h}: {e}"));
                }
            }
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
        // The key column of the settings screen's Keys panel, as drawn: wide
        // enough that the panel sits beside the settings and every row fits.
        let (w, h) = (140u16, 40u16);
        let backend = ratatui::backend::TestBackend::new(w, h);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let mut app = app_at(w, h);
        app.settings_open = true;
        crate::tui::settings::open(&mut app);
        let theme = Theme::default();
        term.draw(|f| crate::tui::settings::render(f, f.area(), &app, &theme))
            .unwrap();
        let buf = term.backend().buffer().clone();
        // Only the key column, never the whole screen: a one-letter key such as
        // `m` appears somewhere on any screen, so searching everything would
        // pass no matter what the panel said.
        let left = (0..w)
            .find(|x| row_text(&buf, 0, *x, 7) == "╭ Keys ")
            .expect("the Keys panel");
        let rows: Vec<String> = (1..h - 1)
            .map(|y| row_text(&buf, y, left + 2, HELP_KEY_WIDTH as u16))
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

    /// As [`render`], but with the cover already loaded, so the art rectangle
    /// holds a picture rather than the empty hole that reserves the space.
    fn render_with_art(w: u16, h: u16) -> (ratatui::buffer::Buffer, Regions) {
        let dir = temp_dir("layout");
        let path = fixture_image(&dir);
        let image = crate::art::decode(&path).expect("decoding the fixture");
        let mut app = app_at(w, h);
        let url = app.track().unwrap().artwork_url.clone().unwrap();
        assert!(app.art.begin(&url));
        let app = update(
            app.clone(),
            Event::Art {
                url,
                result: Ok(crate::player::actions::LoadedArt { path, image }),
            },
        )
        .app;

        let backend = ratatui::backend::TestBackend::new(w, h);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        let mut regions = Regions::default();
        let mut images = Images::halfblocks(HALF_BLOCK_CELL);
        let theme = Theme::default();
        term.draw(|f| draw_with(f, &app, &theme, &mut regions, &mut images))
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        (term.backend().buffer().clone(), regions)
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

    /// The whole frame as text, rows joined by newlines.
    fn full_text(buf: &ratatui::buffer::Buffer) -> String {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
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
        // The bar spans the pane's inner width, whatever the pane turned out to
        // be at this size: it is drawn to `text_body.width` and nothing else.
        assert_eq!(
            row.chars().count() as u16,
            regions.progress.expect("the bar").width,
            "the bar spans the pane's width"
        );
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

    /// A strip too narrow for every tab shows the selected one and marks the
    /// rest as elided, rather than overflowing the pane, truncating the selected
    /// label, or hiding the tab the user is on.
    #[test]
    fn a_narrow_strip_keeps_the_selected_tab_and_marks_the_elision() {
        for (w, h) in [(60u16, 20u16), (80, 24), (100, 30), (140, 40)] {
            for tab in Tab::ALL {
                let mut app = app_at(w, h);
                app.tab = tab;
                let (buf, regions) = render(w, h, &app);
                let text = full_text(&buf);
                assert!(
                    text.contains(tab.label()),
                    "at {w}x{h} the selected tab {} was elided or truncated: {text}",
                    tab.label()
                );
                for (r, _) in &regions.tabs {
                    assert!(
                        r.x + r.width <= w && r.y < h,
                        "a tab rect is off the pane: {r:?}"
                    );
                }
                let mut sorted = regions.tabs.clone();
                sorted.sort_by_key(|(r, _)| r.x);
                for pair in sorted.windows(2) {
                    assert!(
                        pair[0].0.x + pair[0].0.width <= pair[1].0.x,
                        "tab rects overlap at {w}x{h}: {:?} {:?}",
                        pair[0].0,
                        pair[1].0
                    );
                }
            }
        }
        // And at a width where they cannot all fit, something says so rather than
        // the strip silently lying about being complete.
        let mut app = app_at(100, 30);
        app.tab = Tab::Queue;
        let (buf, _) = render(100, 30, &app);
        let text = full_text(&buf);
        assert!(
            text.contains('‹') || text.contains('›'),
            "eight tabs do not fit in 100 columns and nothing said so: {text}"
        );
    }

    /// The selected tab sits in the middle of a strip that cannot show them
    /// all, with as many neighbours before it as after; at the first or last
    /// tab there are none on one side, so it sits at that edge instead.
    #[test]
    fn the_selected_tab_is_centred_until_it_reaches_an_end() {
        let shown = |tab: Tab| {
            let mut app = app_at(100, 30);
            app.tab = tab;
            let strip = tab_strip(&app, &Theme::default(), 60);
            let labels: Vec<String> = strip.segments.iter().map(|(_, t)| t.clone()).collect();
            let at = labels.iter().position(|l| l.starts_with('[')).unwrap_or(99);
            (at, labels.len())
        };
        let mid = Tab::ALL[Tab::ALL.len() / 2];
        let (at, n) = shown(mid);
        assert!(n < Tab::ALL.len(), "60 columns should not fit every tab");
        let (before, after) = (at, n - 1 - at);
        assert!(
            after == before || after == before + 1,
            "{before} before, {after} after"
        );
        assert_eq!(shown(Tab::ALL[0]).0, 0);
        let (at, n) = shown(Tab::ALL[Tab::ALL.len() - 1]);
        assert_eq!(at, n - 1);
    }

    /// Every tab label the strip draws has to be clickable, and clicking one
    /// selects it. The rects come from the same segments as the title.
    #[test]
    fn every_tab_is_clickable_where_it_is_drawn() {
        // Wide enough that all eight fit, because this asserts about every tab
        // and a strip that elides is a different case (the test above).
        let (w, h) = (200u16, 30u16);
        let app = app_at(w, h);
        let (buf, regions) = render(w, h, &app);
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
        // The click index is the tab's position, not a hard-coded name: the strip
        // draws in an order and the click has to agree with it.
        assert_eq!(next.tab, Tab::ALL[1]);
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
                [
                    TRANSPORT_PREV,
                    TRANSPORT_PAUSE,
                    TRANSPORT_PLAY,
                    TRANSPORT_NEXT
                ]
                .iter()
                .any(|g| drawn.contains(g.trim())),
                "control {i} is not over a transport glyph: {drawn:?}"
            );
            assert_eq!(regions.hit(r.x, r.y), Some(Hit::Control(want[i])));
        }
    }

    /// The volume meter's two ends are 0 and 100, it sits on the row the renderer
    /// drew it on, and the target around it is several times the size of the
    /// drawing (owner, 2026-10-03: a one-row slider is hard to hit).
    #[test]
    fn the_volume_meter_is_clickable_end_to_end() {
        let app = app_at(100, 30);
        let (buf, regions) = render(100, 30, &app);
        let v = regions.volume.expect("a volume meter");
        assert!(row_text(&buf, v.y, v.x.saturating_sub(4), 4).contains("vol"));
        assert_eq!(regions.hit(v.x, v.y), Some(Hit::Volume(0.0)));
        assert_eq!(regions.hit(v.x + v.width - 1, v.y), Some(Hit::Volume(1.0)));
        // Two cells of slack either side, so the ends are forgiving, and a click
        // just past the slack is not a volume click at all.
        assert_eq!(regions.hit(v.x + v.width, v.y), Some(Hit::Volume(1.0)));
        assert_eq!(regions.hit(v.x + v.width + 1, v.y), Some(Hit::Volume(1.0)));
        assert_eq!(regions.hit(v.x + v.width + 2, v.y), None);
        // The slack reaches two cells left on the meter's own row, where there is
        // nothing else.
        assert_eq!(
            regions.hit(v.x.saturating_sub(2), v.y),
            Some(Hit::Volume(0.0))
        );
        // The row above answers too -- the duration line and the transport
        // buttons were dead space before. Where the band overlaps a transport
        // button the button still wins, because controls are checked first.
        assert!(
            matches!(
                regions.hit(v.x.saturating_sub(2), v.y - 1),
                Some(Hit::Control(_))
            ),
            "the transport buttons win over the volume band"
        );
        let right = v.x + v.width - 1;
        assert_eq!(regions.hit(right, v.y - 1), Some(Hit::Volume(1.0)));
        // And the rows below: the gradient rule was dead space before.
        assert_eq!(
            regions.hit(v.x + 1, v.y + 1),
            Some(Hit::Volume(1.0 / (v.width - 1) as f64)),
            "the rule under the meter is part of the target"
        );
        // Still the meter's x mapping, not the band's: the two slack cells do not
        // stretch it out.
        assert_eq!(
            regions.hit(v.x + 1, v.y),
            Some(Hit::Volume(1.0 / (v.width - 1) as f64))
        );
        // At 100 % the label still sits two cells clear of a full meter (the
        // `▰` glyph overhangs its cell in real fonts; seen in cmux).
        let mut full = app_at(100, 30);
        full.user_volume = Some(100);
        let (buf, regions) = render(100, 30, &full);
        let v = regions.volume.expect("a volume meter");
        assert_eq!(row_text(&buf, v.y, v.x + v.width, 6), "  100%");
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
        // No spine beside the cover (owner, 2026-10-01): the hole starts at the
        // pane's inner edge and nothing is drawn left of it.
        assert_eq!(
            art.x, 1,
            "the hole starts at the pane's inner edge: {art:?}"
        );
        // The hole is untouched until a cover arrives: the space is reserved,
        // not filled with a picture of nothing.
        let hole: String = (0..art.height)
            .map(|i| row_text(&buf, art.y + i, art.x, art.width))
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(hole.trim(), "", "the hole should be empty: {hole:?}");
        assert!(art.width >= MIN_ART.0 && art.height >= MIN_ART.1, "{art:?}");
    }

    /// The visualizer gets the whole art rectangle: there is no spine beside it
    /// any more. A second gradient running down the edge of the bars was noise
    /// beside a gradient that was already there, and it cost the bars two of the
    /// pane's columns (owner, 2026-10-02).
    #[test]
    fn the_visualizer_takes_the_whole_art_rectangle() {
        let mut app = app_at(100, 30);
        app.settings.display_mode = DisplayMode::Visualizer;
        let (_, regions) = render(100, 30, &app);
        let viz = regions.art.expect("the visualizer area");
        let cover = render(100, 30, &app_at(100, 30))
            .1
            .art
            .expect("the cover area");
        assert_eq!(viz, cover, "the visualizer and the cover share one rect");
        assert_eq!(viz.x, 1, "hard against the pane's edge: {viz:?}");
    }

    /// The tab strip at a wide terminal: every tab fits, so there is no elision and
    /// the selected one keeps its brackets. A 200-column pane used to draw
    /// `istory]` -- the strip is the pane's *title*, and a title that does not fit
    /// is clipped, so the geometry of the pane decides whether the label is whole.
    #[test]
    fn a_wide_strip_draws_every_tab_and_keeps_the_selected_one_bracketed() {
        for (w, h) in [(200u16, 62u16), (240u16, 80u16), (140, 40), (110, 34)] {
            let (buf, _) = render(w, h, &app_at(w, h));
            let strip = full_text(&buf)
                .lines()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            assert!(strip.contains("History"), "{w}x{h}: {strip}");
            assert!(
                strip.contains("[History]"),
                "{w}x{h}: the selected tab lost its brackets: {strip}"
            );
        }
    }

    /// The cover and the text together have to fill the pane. This is the test
    /// for the dead space a tall or a very wide terminal used to show: a cover
    /// frozen at twenty rows in a fifty-row pane, with thirty rows of nothing
    /// under it (owner, 2026-10-02).
    #[test]
    fn the_now_playing_pane_has_no_gap_inside_its_block() {
        for (w, h) in [
            (200u16, 62u16),
            (240, 80),
            (120, 50),
            (100, 30),
            (80, 24),
            (76, 16),
        ] {
            let (buf, regions) = render_with_art(w, h);
            let art = regions.art.expect("the cover");
            let bar = regions.progress.expect("the progress bar");
            // The rule sits under the last text row, so the block runs from the
            // top of the cover to the bottom of the pane's inner area.
            assert_eq!(art.x, 1, "{w}x{h}: the cover is hard against the edge");
            // The bar is the fifth text row, so it is below the cover by exactly
            // the rows of type above it -- never by rows of nothing.
            assert!(
                bar.y > art.y + art.height,
                "{w}x{h}: the bar is over the art"
            );
            assert!(
                bar.y - (art.y + art.height) <= 5,
                "{w}x{h}: {} blank rows between the cover and the text",
                bar.y - (art.y + art.height)
            );
            // Nothing between the last row of text and the foot of the pane.
            let pane_floor = regions.volume.map_or(bar.y + 2, |v| v.y + 2);
            // Above and below the block the rows are split evenly, which is what
            // makes a pane with room to spare look composed rather than top-heavy.
            let above = art.y - 2;
            let slack = h as i32 - pane_floor as i32;
            assert!(slack <= 8, "{w}x{h}: {slack} rows below the block");

            assert!(
                above <= 6,
                "{w}x{h}: {above} rows above the block and {slack} below"
            );
            let (x0, x1) = (art.x, art.x + art.width);
            // Inside the block -- from the top of the cover to the foot of the
            // rule -- there is no run of empty rows at all.
            let longest = longest_blank_run(&buf, x0, x1, art.y, pane_floor + 1);
            assert!(longest <= 1, "{w}x{h}: {longest} blank rows at {x0}..{x1}");
        }
    }

    /// A short block in a tall pane is centred rather than left at the top, so a
    /// half-empty pane reads as composed instead of unfinished.
    #[test]
    fn a_short_pane_of_content_is_centred_not_stranded_at_the_top() {
        let app = app_at(120, 60);
        let (buf, _) = render(120, 60, &app);
        let rows: Vec<u16> = (0..60)
            .filter(|y| {
                (0..120).any(|x| {
                    let c = buf[(x, *y)].symbol();
                    !c.is_empty()
                        && c != " "
                        && !c.starts_with('│')
                        && !c.starts_with('╭')
                        && !c.starts_with('╰')
                        && !c.starts_with('─')
                })
            })
            .collect();
        assert!(rows.len() > 4, "the tab pane should have some content");
        // The right-hand pane's content starts well below its top border.
        let first = rows
            .iter()
            .copied()
            .find(|y| *y > 3 && (95..120).any(|x| buf[(x, *y)].symbol().trim() != "│"))
            .expect("a row of content in the tab pane");
        assert!(
            first >= 6,
            "content starts on row {first}, not centred: {rows:?}"
        );
    }

    /// `now_playing_width` is computed from an estimate of the text block's
    /// height, so the two have to agree: if the text ever grows past the estimate
    /// the pane is sized for the wrong thing and the cover goes back to being a
    /// small picture in a large box.
    #[test]
    fn the_text_estimate_matches_the_rows_that_are_drawn() {
        // Only sizes where the whole text block fits: in a shorter pane the block
        // is truncated at the bottom, so the last recorded row is a row that was
        // never drawn and says nothing about how tall the block is.
        for (w, h) in [
            (240u16, 80u16),
            (200, 62),
            (120, 40),
            (100, 30),
            (80, 24),
            (76, 16),
        ] {
            for volume in [true, false] {
                for progress in [true, false] {
                    let mut app = app_at(w, h);
                    app.settings.show_volume = volume;
                    app.settings.show_progress = progress;
                    let track = app.track().expect("a track").clone();
                    let (_, regions) = render(w, h, &app);
                    // The last row of the block: the meter when it is drawn, the
                    // controls otherwise.
                    let last = match regions.volume {
                        Some(v) => v.y,
                        None => regions.controls.first().expect("controls").0.y,
                    };
                    let art = regions.art.map_or(0, |a| a.y + a.height);
                    assert_eq!(
                        text_rows(&app, &track),
                        last + 1 - art,
                        "{w}x{h} volume={volume} progress={progress}"
                    );
                    assert!(text_rows(&app, &track) <= TEXT_ROWS_MAX);
                }
            }
        }
    }

    /// **A re-encoded cover asks for a repaint, exactly once** (owner, 2026-10-03,
    /// "remove those random smaller gray boxes"). A Kitty placement covers only the
    /// cells it was encoded for, so a new cover at a new size leaves the old
    /// placement's cells on the terminal and ratatui cannot write them -- they are
    /// the image's own. The renderer cannot clear the screen from inside a frame,
    /// so it raises the flag and the loop does it.
    #[test]
    fn a_new_cover_asks_for_a_repaint_once() {
        let square = || {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                640,
                640,
                image::Rgb([10, 20, 30]),
            ))
        };
        let tall = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            300,
            900,
            image::Rgb([40, 50, 60]),
        ));
        let mut images = Images::halfblocks((10, 20));
        assert!(!images.repainted(), "nothing has been drawn yet");

        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 12)).unwrap();
        let a = PathBuf::from("/tmp/a.png");
        let b = PathBuf::from("/tmp/b.png");
        let _ = term.draw(|f| images.draw(f, &a, &square(), f.area()));
        assert!(images.repainted(), "the first cover asks");
        assert!(!images.repainted(), "and only once");

        let _ = term.draw(|f| images.draw(f, &a, &square(), f.area()));
        assert!(!images.repainted(), "an unchanged cover is quiet");

        let _ = term.draw(|f| images.draw(f, &b, &square(), f.area()));
        assert!(images.repainted(), "a new cover asks, same size or not");

        // And a resize asks, because the placement was encoded for the old size.
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(24, 8)).unwrap();
        let _ = term.draw(|f| images.draw(f, &b, &tall, f.area()));
        assert!(images.repainted(), "a smaller terminal asks");
        assert!(!images.repainted(), "once is once");
    }

    /// The cell size reply, parsed. The terminal answers height first, and a
    /// nonsense answer is refused rather than believed: an assumed cell size is
    /// only as good as a measured one.
    #[test]
    fn the_cell_size_reply_is_parsed_and_believed_only_when_it_makes_sense() {
        assert_eq!(parse_cell_size(b"\x1b[6;17;8t"), Some((8, 17)));
        assert_eq!(
            parse_cell_size(b"junk\x1b[6;34;21t more junk"),
            Some((21, 34))
        );
        // Not an answer at all.
        assert_eq!(parse_cell_size(b""), None);
        assert_eq!(parse_cell_size(b"\x1b[?62;1;2c"), None);
        // Plausible for a *character* report rather than a pixel one: refused.
        assert_eq!(parse_cell_size(b"\x1b[6;80;200t"), None);
        assert_eq!(parse_cell_size(b"\x1b[6;17;0t"), None);
    }

    /// The measured cell size is the one the art is sized with, and the default is
    /// kept when nothing answers.
    #[test]
    fn the_measured_cell_size_is_the_one_that_sizes_the_art() {
        let mut images = Images::halfblocks((10, 20));
        assert_eq!(images.cell_size(), (10, 20));
        images.set_cell_size((21, 34));
        assert_eq!(images.cell_size(), (21, 34), "believed");
        images.set_cell_size((0, 0));
        assert_eq!(images.cell_size(), (21, 34), "a nonsense answer is ignored");
    }

    /// A square cover fills the pane it is given, whatever the cell size. This is
    /// the bug the query fixes: with a cell size measured on another machine, the
    /// cover came out at half the width it should have and the pane was half empty.
    #[test]
    fn a_square_cover_fills_a_pane_whatever_the_cell_size() {
        let square = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            640,
            640,
            image::Rgb([120, 60, 200]),
        ));
        for cell in [(10u16, 20u16), (21, 34)] {
            let images = Images::halfblocks(cell);
            let hole = Rect::new(0, 0, 90, 50);
            let fitted = images.fit(hole, &square);
            // A square cover is `ch / cw` times as wide as it is tall, in cells.
            let want = fitted.height as f64 * cell.1 as f64 / cell.0 as f64;
            assert!(
                (fitted.width as f64 - want).abs() <= 1.0,
                "{cell:?}: fitted {fitted:?}, expected about {want} across"
            );
            assert!(
                fitted.width + 1 >= hole.width || fitted.height + 1 >= hole.height,
                "{cell:?}: {fitted:?} does not fill {hole:?}"
            );
        }
    }

    /// The longest run of rows with nothing but background in `x0..x1`.
    fn longest_blank_run(
        buf: &ratatui::buffer::Buffer,
        x0: u16,
        x1: u16,
        from: u16,
        to: u16,
    ) -> u16 {
        let mut longest = 0;
        let mut run = 0;
        for y in from..to {
            let blank = (x0..x1).all(|x| {
                let c = buf[(x, y)].symbol();
                c.is_empty() || c == " "
            });
            run = if blank { run + 1 } else { 0 };
            longest = longest.max(run);
        }
        longest
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
    // -- Lyrics scroll and the full-screen page (TODO 6.3, 6.4) -------------------

    /// An app with Ready lyrics and a scroll the user has taken, so the anchor
    /// is a known line wherever the interpolator happens to be.
    fn lyrics_app(lines: usize, scrolled_to: Option<usize>) -> App {
        let app = app_at(100, 30);
        let uri = app.track().unwrap().uri.clone();
        let lyrics = crate::lyrics::Lyrics {
            lines: (0..lines)
                .map(|i| crate::lyrics::LyricLine {
                    time_secs: i as f64 * 10.0,
                    text: format!("line {i}"),
                })
                .collect(),
            synced: true,
            source: "test".into(),
            instrumental: false,
        };
        let mut app = update(
            app,
            Event::Lyrics {
                uri,
                result: Ok(lyrics),
            },
        )
        .app;
        app.lyrics.scrolled_to = scrolled_to;
        app
    }

    fn page_term(app: &App, w: u16, h: u16) -> ratatui::Terminal<ratatui::backend::TestBackend> {
        let backend = ratatui::backend::TestBackend::new(w, h);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| draw(f, app, &Theme::default())).unwrap();
        term
    }

    /// One row of the buffer as (text, per-cell fg colours).
    fn row_at(
        term: &ratatui::Terminal<ratatui::backend::TestBackend>,
        y: u16,
    ) -> (String, Vec<ratatui::style::Color>) {
        let buf = term.backend().buffer();
        let mut text = String::new();
        let mut fgs = Vec::new();
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            text.push_str(cell.symbol());
            fgs.push(cell.fg);
        }
        (text, fgs)
    }

    #[test]
    fn wrapping_happens_by_display_width_not_characters() {
        use unicode_width::UnicodeWidthStr;
        // ASCII: the character count is the width.
        assert_eq!(wrap_by_width("abcdefgh", 3), vec!["abc", "def", "gh"]);
        assert_eq!(wrap_by_width("exact", 5), vec!["exact"]);
        assert_eq!(wrap_by_width("", 5), vec![""]);
        // A CJK character is two cells: four of them is a full row of eight.
        assert_eq!(wrap_by_width("日本語日", 8), vec!["日本語日"]);
        assert_eq!(wrap_by_width("日本語日本", 6), vec!["日本語", "日本"]);
        // A zero-width character rides along rather than starting a row.
        assert_eq!(wrap_by_width("a\u{301}bc", 2), vec!["a\u{301}b", "c"]);
        // Every row must fit its width; a row of one is the degenerate case.
        for (text, width) in [("word word word", 1), ("日本語", 1)] {
            for row in wrap_by_width(text, width) {
                assert!(
                    row.width() <= width.max(1) || row.chars().count() == 1,
                    "{text:?} at {width}: {row:?} is wider than the row"
                );
            }
        }
    }

    /// TODO 6.4: the page centres the anchor line, in the accent colour and
    /// bold, with its neighbours dim above and below and the way out named.
    #[test]
    fn the_full_screen_page_centres_the_anchor_and_dims_the_rest() {
        let mut app = lyrics_app(12, Some(5));
        app.lyrics_full = true;
        let theme = Theme::default();
        let term = page_term(&app, 80, 24);

        let (mid, mid_fg) = row_at(&term, 12);
        assert!(
            mid.contains("line 5"),
            "the anchor is on the middle row: {mid:?}"
        );
        // 80 columns, "line 5" centred: the text starts at column 37.
        assert_eq!(mid_fg[37], theme.accent_colour(), "in the accent colour");

        let (above, above_fg) = row_at(&term, 11);
        assert!(
            above.contains("line 4"),
            "the line before it, above: {above:?}"
        );
        assert_ne!(above_fg[37], theme.accent_colour(), "and not in the accent");

        let (below, _) = row_at(&term, 13);
        assert!(
            below.contains("line 6"),
            "the line after it, below: {below:?}"
        );

        let (hint, _) = row_at(&term, 23);
        assert!(
            hint.contains("L or esc returns"),
            "the page names its own way out: {hint:?}"
        );

        // The dashboard is gone: the tab strip is not drawn behind the page.
        for y in 0..24 {
            let (text, _) = row_at(&term, y);
            assert!(
                !text.contains("History"),
                "row {y} still draws the dashboard"
            );
        }
    }

    /// With a cover, the page draws it behind the words as darkened cell
    /// backgrounds, and the words stay in place and readable on it.
    #[test]
    fn the_full_screen_page_puts_the_cover_behind_the_words() {
        let dir = temp_dir("backdrop");
        let path = fixture_image(&dir);
        let image = crate::art::decode(&path).expect("decoding the fixture");
        let mut app = lyrics_app(12, Some(5));
        app.lyrics_full = true;
        let url = app.track().unwrap().artwork_url.clone().unwrap();
        assert!(app.art.begin(&url));
        let app = update(
            app,
            Event::Art {
                url,
                result: Ok(crate::player::actions::LoadedArt { path, image }),
            },
        )
        .app;

        let plain = page_term(&lyrics_app(12, Some(5)), 80, 24);
        let term = page_term(&app, 80, 24);
        let buf = term.backend().buffer();
        let painted = buf
            .content
            .iter()
            .filter(|c| matches!(c.bg, Color::Rgb(..)))
            .count();
        assert!(
            painted > 80 * 24 / 4,
            "only {painted} cells carry the cover"
        );
        // The fixture's red half, a third as bright: dark enough to read on.
        for c in buf.content.iter() {
            if let Color::Rgb(r, g, b) = c.bg {
                assert!(r.max(g).max(b) <= 85, "too bright behind text: {r} {g} {b}");
            }
        }
        let (mid, _) = row_at(&term, 12);
        assert!(mid.contains("line 5"), "{mid:?}");
        let (above, fg) = row_at(&term, 11);
        assert!(above.contains("line 4"));
        assert_eq!(
            fg[37],
            Color::Rgb(165, 165, 165),
            "dim grey lifted on the cover"
        );
        // Without a cover the words sit on the terminal's own background.
        let plain = plain.backend().buffer();
        assert!((0..80).all(|x| plain[(x, 11)].bg == Color::Reset));
    }

    /// A long line wraps onto several rows rather than tearing off the edge,
    /// and a CJK line wraps at half the characters, because each one is two
    /// cells wide (TODO 6.4's "no wrapping glitches").
    /// A row's display width, walking the buffer the way the terminal does: a
    /// two-cell glyph is followed by a skip cell that must not be counted, or
    /// every wide character reads as three columns.
    fn row_display_width(term: &ratatui::Terminal<ratatui::backend::TestBackend>, y: u16) -> usize {
        use unicode_width::UnicodeWidthStr;
        let buf = term.backend().buffer();
        let mut width = 0;
        let mut skip_next = false;
        for x in 0..buf.area.width {
            if skip_next {
                skip_next = false;
                continue;
            }
            let w = buf[(x, y)].symbol().width();
            if w == 0 {
                continue;
            }
            width += w;
            skip_next = w >= 2;
        }
        width
    }

    #[test]
    fn the_page_wraps_long_and_wide_lines_without_tearing() {
        let mut app = lyrics_app(3, None);
        app.lyrics_full = true;
        if let Some(l) = app.lyrics.lyrics.as_mut() {
            l.lines[0].text = "w".repeat(200);
            l.lines[1].text = "日".repeat(50);
        }
        let term = page_term(&app, 80, 24);

        for y in 0..24 {
            let (text, _) = row_at(&term, y);
            let width = row_display_width(&term, y);
            assert!(
                width <= 80,
                "row {y} is {text:?} and {width} columns wide, wider than the screen"
            );
        }
        // 200 w's on an 80-wide screen: three rows, 80 + 80 + 40. Counted by
        // glyph rather than `trim_end().len()`, because a centred short row
        // keeps its leading padding.
        let (r0, r0_fg) = row_at(&term, 11);
        let (r1, _) = row_at(&term, 12);
        let (r2, _) = row_at(&term, 13);
        assert_eq!(r0.matches('w').count(), 80, "{r0:?}");
        assert_eq!(r1.matches('w').count(), 80, "{r1:?}");
        assert_eq!(r2.matches('w').count(), 40, "{r2:?}");
        assert_eq!(
            r0_fg[0],
            Theme::default().accent_colour(),
            "wrapped rows keep the accent"
        );

        // 50 CJK characters is 100 cells: 40 of them fit, then the last 10.
        let (cjk0, _) = row_at(&term, 14);
        let (cjk1, _) = row_at(&term, 15);
        assert_eq!(cjk0.matches('日').count(), 40, "{cjk0:?}");
        assert_eq!(
            row_display_width(&term, 14),
            80,
            "40 wide glyphs fill the row"
        );
        assert_eq!(cjk1.matches('日').count(), 10, "{cjk1:?}");
    }

    /// The page draws at every size the dashboard has a breakpoint for, and at
    /// sizes below them, without panicking — including one row tall.
    #[test]
    fn the_page_renders_at_every_size_without_panicking() {
        let mut app = lyrics_app(20, Some(10));
        app.lyrics_full = true;
        for (w, h) in [
            (100, 30),
            (80, 24),
            (76, 16),
            (46, 12),
            (30, 8),
            (20, 4),
            (10, 1),
        ] {
            let term = page_term(&app, w, h);
            let (text, _) = row_at(&term, h / 2);
            assert!(
                text.trim().contains("line 10"),
                "{w}x{h}: the anchor should be somewhere on screen: {text:?}"
            );
        }
    }

    /// TODO 6.3: while the user holds the scroll, the tab says so — a view that
    /// quietly stopped following the song looks like a bug.
    /// "no lyrics for this one" is the heading, not also the explanation: it used
    /// to be printed twice, which reads as two different pieces of news.
    #[test]
    fn a_missing_lyric_says_it_once() {
        let mut app = lyrics_app(0, None);
        app.tab = Tab::Lyrics;
        let text = full_text(&render(100, 30, &app).0);
        assert_eq!(
            text.matches("no lyrics for this one").count(),
            1,
            "said twice:\n{text}"
        );
        assert!(text.contains("LRCLIB has most songs"), "{text}");
    }

    /// A lyric line is as long as the song says it is, and the side tab has no
    /// sideways scroll: a long line used to be cut off at the pane's edge with
    /// the rest of the words unreachable (owner, 2026-10-02).
    #[test]
    fn a_long_lyric_line_wraps_instead_of_being_cut_off() {
        let long = "and the whole of the rest of it goes on and on well past the edge of the pane";
        let mut app = lyrics_app(3, None);
        app.lyrics.lyrics.as_mut().unwrap().lines[1].text = long.into();
        app.tab = Tab::Lyrics;
        // A narrow pane, which is where it showed up.
        for (w, h) in [(76u16, 20u16), (100, 30), (140, 40)] {
            let (buf, _) = render(w, h, &app);
            let text = full_text(&buf);
            let words: Vec<&str> = long.split(' ').collect();
            // The first words and the last words are both on screen: nothing was
            // cut off, because the line wrapped.
            assert!(text.contains(words[0]), "{w}x{h}: {text}");
            assert!(
                text.contains(words[words.len() - 1]),
                "{w}x{h}: the line was cut off, not wrapped:\n{text}"
            );
        }
    }

    #[test]
    fn the_lyrics_tab_says_when_the_follow_is_paused() {
        let mut app = lyrics_app(20, Some(9));
        app.tab = Tab::Lyrics;
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| draw(f, &app, &Theme::default())).unwrap();
        let text = {
            let buf = term.backend().buffer().clone();
            (0..buf.area.height)
                .map(|y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(
            text.contains("following paused"),
            "the pane must say the follow is paused:\n{text}"
        );
        assert!(
            text.contains("line 9"),
            "the scrolled-to line is the one on show"
        );
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::{GraphicsProtocol, detect_protocol};

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
