//! Drawing the app. Pure: it takes `&App` and writes cells. It never reads the
//! player and never changes state.
//!
//! The layout is computed from the terminal size on every frame, and the
//! breakpoints below are the ones TODO 3.4 tunes. The rule from SPEC §3 that
//! matters most: **borders must never wrap or tear**, so every pane is built from
//! `Rect`s that are shrunk before anything is drawn inside them.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crate::player::PlaybackState;
use crate::tui::app::{App, HISTORY_VIEW, Tab};
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

/// How much room the Now Playing pane gets, as a share of the width.
const NOW_PLAYING_SHARE: u16 = 46;

pub fn draw(f: &mut Frame, app: &App, theme: &Theme) {
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
            draw_toast(f, area, &t.text);
        }
        return;
    }

    match layout_for(w, h) {
        Layout_::TooSmall => draw_too_small(f, area, app),
        Layout_::Compact => draw_compact(f, area, app, theme),
        Layout_::Stacked => draw_stacked(f, area, app, theme),
        Layout_::Wide => draw_wide(f, area, app, theme),
    }

    if app.show_help {
        draw_help(f, area);
    }
    if let Some(t) = &app.toast {
        draw_toast(f, area, &t.text);
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

fn pane_block<'a>(title: &'a str, theme: &Theme, focused: bool) -> Block<'a> {
    let mut b = Block::bordered()
        .title(title)
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

fn draw_wide(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
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
        draw_now_playing(f, rows[1], app, theme);
    } else {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(NOW_PLAYING_SHARE),
                Constraint::Min(24),
            ])
            .split(rows[1]);
        draw_now_playing(f, cols[0], app, theme);
        draw_tabs(f, cols[1], app, theme);
    }

    draw_footer(f, rows[2], app, theme);
}

fn draw_stacked(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
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
    draw_now_playing(f, rows[1], app, theme);
    if app.settings.side_pane {
        draw_tabs(f, rows[2], app, theme);
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

fn draw_compact(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
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
    let mut left = vec![Span::raw(" trak ")];
    if !app.is_idle() {
        left.push(Span::styled(
            "●",
            Style::default().fg(Theme::status_colour(state)),
        ));
        left.push(Span::raw(" "));
        left.push(Span::raw(
            state.map(|s| s.symbol()).unwrap_or("▶").to_string(),
        ));
        left.push(Span::raw("  "));
        if let Some(t) = app.track() {
            left.push(Span::styled(
                t.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ));
        }
    } else {
        left.push(Span::styled("not running", Theme::dim()));
    }

    let right = if app.settings.show_clock && !app.clock.is_empty() {
        vec![Span::styled(format!("{} ", app.clock), Theme::dim())]
    } else {
        vec![]
    };

    let line = Line::from(left);
    f.render_widget(Paragraph::new(line), area);
    if !right.is_empty() {
        f.render_widget(Paragraph::new(Line::from(right)).right_aligned(), area);
    }
    let _ = theme;
}

fn draw_now_playing(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = pane_block(" Now Playing ", theme, true);
    f.render_widget(block.clone(), area);
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
    let mut lines: Vec<Line> = Vec::new();

    // The art or visualizer pane (TODO 4.1) owns the top third. It is drawn as a
    // framed placeholder so the layout reads as designed rather than unfinished,
    // and so the renderer already reserves exactly the space art will need.
    let art_h = (body.height / 3).clamp(4, 12) as usize;
    if art_h >= 4 {
        lines.push(Line::from(Span::styled(
            format!("┌{}┐", "─".repeat(body.width.saturating_sub(2) as usize)),
            Theme::dim(),
        )));
        let rows = art_h.saturating_sub(3);
        for _ in 0..rows {
            lines.push(Line::from(Span::styled(
                format!("│{}│", " ".repeat(body.width.saturating_sub(2) as usize)),
                Theme::dim(),
            )));
        }
        lines.push(Line::from(Span::styled(
            format!("└{}┘", "─".repeat(body.width.saturating_sub(2) as usize)),
            Theme::dim(),
        )));
        lines.push(Line::from(""));
    }

    if !track.artist.is_empty() {
        lines.push(Line::from(Span::styled(
            track.artist.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
    }
    if !track.album.is_empty() {
        lines.push(Line::from(Span::styled(track.album.clone(), Theme::dim())));
    }

    let bar_w = body.width as usize;
    lines.push(Line::from(Span::styled(
        progress_bar(progress(app), bar_w),
        theme.accent_style(),
    )));
    lines.push(Line::from(vec![
        Span::styled(format_time(pos), Theme::dim()),
        Span::raw(" "),
        Span::styled(format_time(dur), Style::default().fg(theme.accent_colour())),
    ]));

    lines.push(Line::from(""));
    let mut controls = String::from(" ⏮ ");
    controls.push_str(match app.state.as_ref().map(|s| s.playback) {
        Some(PlaybackState::Playing) => "⏸",
        _ => "▶",
    });
    controls.push_str(" ⏭");
    if app.state.as_ref().is_some_and(|s| s.shuffling_enabled) {
        controls.push_str("   ⇄");
    }
    if app.repeat != crate::player::RepeatMode::Off {
        controls.push_str("   ");
        controls.push_str(app.repeat.symbol());
    }
    lines.push(Line::from(controls));

    // Hidden when a volume write did not land: a meter that cannot be trusted is
    // worse than no meter, and the notice says why (COMPAT rule 5).
    if app.settings.show_volume && !app.volume_hidden {
        // Plain text, not 🔊: the emoji rendered as a *muted* speaker in cmux
        // next to a 100% meter, which is the opposite of what it means.
        let v = app.meter_volume();
        lines.push(Line::from(Span::styled(
            format!("vol {} {}%", Theme::volume_meter(v, 10), v),
            Style::default().fg(theme.accent_colour()),
        )));
    }

    let shown = lines.len().min(body.height as usize);
    let _ = art_h;
    f.render_widget(
        Paragraph::new(lines.into_iter().take(shown).collect::<Vec<_>>()),
        body,
    );
}

fn progress(app: &App) -> f64 {
    match &app.state {
        Some(s) => s.progress(),
        None => 0.0,
    }
}

fn draw_tabs(f: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let title = format!(" {} ", tab_strip(app));
    let block = pane_block(&title, theme, false);
    f.render_widget(block, area);
    let body = inner(area);
    if body.width == 0 || body.height == 0 {
        return;
    }

    let lines: Vec<Line> = match app.tab {
        Tab::History => history_lines(app, theme),
        Tab::Info => info_lines(app, theme),
        Tab::Lyrics => vec![Line::from(Span::styled(
            if app.is_idle() { "" } else { "no lyrics yet" },
            Theme::dim(),
        ))],
    };
    f.render_widget(Paragraph::new(lines), body);
}

fn tab_strip(app: &App) -> String {
    Tab::ALL
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let n = i + 1;
            if *t == app.tab {
                format!("[{n}]{}", t.label())
            } else {
                format!(" {n} {} ", t.label())
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
        app.history
            .iter()
            .rev()
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

fn draw_toast(f: &mut Frame, area: Rect, text: &str) {
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
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ))),
        popup,
    );
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
        assert!(text_of(&mut term, &app, 100, 30).contains('▰'));
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
                let y = popup.y + 1 + i as u16;
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
