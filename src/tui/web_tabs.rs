//! The five Web API tabs, and the pages you open out of them (TODO 7.6-7.10).
//!
//! `render.rs` draws one of these in place of the History/Info/Lyrics lines, so
//! everything here is a function of `&App` and `&Theme` that hands back text: no
//! requests, no cursor, no clock. The fetching belongs to the event loop and the
//! cursor to `update`, and this file may not take either back.
//!
//! Two things from `docs/WEB-API.md` shape every line below.
//!
//! **Four of the first plan's assumptions are stale in dev mode** (§0): artist
//! top tracks are gone with no replacement, track and album popularity are gone,
//! and a playlist's `items` come back only for a playlist the user owns or
//! collaborates on. Nothing here offers them, and where a person would go
//! looking for one, a line says why it is not there.
//! **The Web API is optional**, which is why the connection notice is the first
//! line of every tab and not a toast that has already faded: a person with no
//! Client ID who lands on an empty Search pane has no way to tell a feature
//! from a bug, and this is the only place that can tell them apart.
//!
//! Nothing is truncated here. `lines` does not know how wide the pane is, so
//! every row is the whole row and `Paragraph` clips it -- the same bargain
//! `history_lines` and `info_lines` keep, and the reason a long title scrolls in
//! the Now Playing pane rather than being cut to fit in a tab.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::app::{App, LibrarySection, Open, PlaylistEdit, Tab};
use crate::tui::theme::Theme;
use crate::web::api::{Page, Queue, SearchResults};

/// The four result groups, in the order they are drawn.
///
/// A count rather than a name so the headings and the rows cannot disagree about
/// what order they are in, and so `group_row` has one index per heading.
const GROUPS: [&str; 4] = ["Tracks", "Albums", "Artists", "Playlists"];

/// How many songs the Queue tab shows when the Web API has no queue to show.
const RECENT_ON_QUEUE: usize = 12;

/// The lines for whichever Web API tab is showing. Called from `render.rs` in
/// place of the History/Info/Lyrics lines.
pub fn lines(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    // The playlist modal replaces the body, like an opened page does (7.11).
    if let Some(edit) = &app.web.edit {
        return edit_lines(app, theme, edit);
    }
    let mut out = Vec::new();
    if let Some(notice) = app.web.connection.notice() {
        // First, bold, and in the tab itself. `update` toasts this too, but a
        // toast is gone in two and a half seconds and this is the difference
        // between "trak is broken" and "there is one thing to do here".
        out.push(Line::from(Span::styled(
            notice,
            theme.accent_style().add_modifier(Modifier::BOLD),
        )));
        out.push(Line::from(""));
    }
    // A page replaces the list it was opened from rather than being drawn under
    // it: two lists and two cursors on one pane is a pane where the key the user
    // pressed does something to a list they cannot see.
    if let Some(open) = &app.web.open {
        out.extend(page_lines(app, theme, open));
        return out;
    }
    out.extend(match app.tab {
        Tab::Search => search_lines(app, theme),
        Tab::Playlists => playlist_lines(app),
        Tab::Queue => queue_lines(app, theme),
        Tab::Liked => liked_lines(app, theme),
        Tab::Library => library_lines(app, theme),
        // History, Info and Lyrics come from AppleScript and LRCLIB, and
        // `render.rs` never sends them here. The arm is there so a ninth tab
        // cannot take the pane down with it.
        _ => vec![hint("  this tab does not use the Spotify Web API")],
    });
    out
}

// ---------------------------------------------------------------------------
// Search (7.6)
// ---------------------------------------------------------------------------

/// The query box and the four groups under it.
fn search_lines(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    let web = &app.web;
    let mut out = vec![query_box(app, theme)];
    out.extend(search_status(app));
    // A cursor past the last group would be a panic here and a blank heading
    // everywhere else; the last heading is a better answer than either.
    let current = web.group.min(GROUPS.len() - 1);
    for (i, name) in GROUPS.iter().enumerate() {
        out.push(group_heading(
            name,
            group_len(&web.results, i),
            theme,
            i == current,
        ));
        for (row, text) in group_rows(&web.results, i).into_iter().enumerate() {
            out.push(row_line(&text, row == web.group_row[i]));
        }
    }
    out
}

/// The box, with a caret at the end of the query while it has the focus.
///
/// The caret is a character rather than a reversed space because a reversed
/// space is invisible in a `TestBackend` buffer and in a screenshot, and a
/// cursor nobody can see is the same as no focus at all.
fn query_box(app: &App, theme: &Theme) -> Line<'static> {
    let mut spans = vec![
        // The key that focuses it, in the place the key is typed, so the box
        // looks like an input rather than a label.
        Span::styled("/ ", Theme::dim()),
        Span::raw(app.web.query.clone()),
    ];
    if app.web.search_focus {
        spans.push(Span::styled(
            "▏",
            theme.accent_style().add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

/// What to say above the groups: still looking, nothing to look for, or nothing
/// found. `None` when the groups below are the whole answer.
fn search_status(app: &App) -> Option<Line<'static>> {
    if app.web.searching {
        // On the row the results would go, so a refresh does not move
        // everything under it.
        return Some(hint("  searching…"));
    }
    if app.web.query.trim().is_empty() {
        return Some(hint("  type to search — / focuses the box"));
    }
    if app.web.search_shown.trim().is_empty() {
        // Typed but not asked yet: the debounce is still holding the request.
        return None;
    }
    if app.web.results.is_empty() {
        return Some(hint(format!(
            "  nothing matched \"{}\"",
            app.web.search_shown
        )));
    }
    None
}

/// How many results a group has, or `0` for an index that is not one of the four.
fn group_len(results: &SearchResults, group: usize) -> usize {
    match group {
        0 => results.tracks.len(),
        1 => results.albums.len(),
        2 => results.artists.len(),
        _ => results.playlists.len(),
    }
}

/// A group's rows, already in the text the rest of trak shows a track in -- the
/// `Display` impls in `web::api` are what the API client's tests agree with, so
/// a tab row and the fake say the same thing character for character.
fn group_rows(results: &SearchResults, group: usize) -> Vec<String> {
    match group {
        0 => results.tracks.iter().map(|t| t.to_string()).collect(),
        1 => results.albums.iter().map(|a| a.to_string()).collect(),
        2 => results.artists.iter().map(|a| a.to_string()).collect(),
        _ => results.playlists.iter().map(|p| p.to_string()).collect(),
    }
}

/// A group heading with its count.
///
/// A group with nothing in it keeps its heading, in `dim`, and a group with
/// something in it is picked out when the cursor is in it. Hiding an empty group
/// would make the answer jump about as the query narrows, and a person reading
/// the pane could no longer see what had been searched for.
fn group_heading(name: &str, count: usize, theme: &Theme, current: bool) -> Line<'static> {
    let style = if current && count > 0 {
        theme.accent_style().add_modifier(Modifier::BOLD)
    } else {
        Theme::dim()
    };
    Line::from(Span::styled(format!("  {name} ({count})"), style))
}

// ---------------------------------------------------------------------------
// Playlists (7.7)
// ---------------------------------------------------------------------------

fn playlist_lines(app: &App) -> Vec<Line<'static>> {
    let page = &app.web.playlists;
    if page.items.is_empty() {
        let mut out = vec![hint("  no playlists yet")];
        out.extend(more(page));
        return out;
    }
    let mut unreadable = false;
    let mut out: Vec<Line<'static>> = page
        .items
        .iter()
        .enumerate()
        .map(|(i, playlist)| {
            let text = playlist.to_string();
            // `items` is absent for any playlist the user neither owns nor
            // collaborates on ("For other playlists, only metadata is returned
            // and the `items` field will be absent"), so a row that says nothing
            // about it would be promising a tracklist `enter` cannot open.
            match playlist.contents {
                Some(_) => row_line(&text, i == app.web.playlist_cursor),
                None => {
                    unreadable = true;
                    row_line(
                        &format!("{text}  (no tracklist)"),
                        i == app.web.playlist_cursor,
                    )
                }
            }
        })
        .collect();
    out.extend(more(page));
    if unreadable {
        out.push(hint(
            "  a tracklist comes back only for playlists you own or collaborate on",
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Playlist editing (7.11)
// ---------------------------------------------------------------------------

/// What the modal says. Plain lines so a test can read them.
fn edit_lines(app: &App, theme: &Theme, edit: &PlaylistEdit) -> Vec<Line<'static>> {
    let bold = theme.accent_style().add_modifier(Modifier::BOLD);
    match edit {
        PlaylistEdit::Pick { uri, cursor } => {
            let what = app
                .track()
                .filter(|t| t.uri.as_deref() == Some(uri.as_str()))
                .map(|t| format!("{} — {}", t.artist, t.title))
                .unwrap_or_else(|| "the selected track".to_string());
            let mut out = vec![
                Line::from(Span::styled(format!("Add {what} to which playlist?"), bold)),
                hint("  enter adds · n makes a new playlist · esc cancels"),
                Line::from(""),
            ];
            let page = &app.web.playlists;
            if page.items.is_empty() {
                out.push(hint("  loading your playlists... (or n to make the first)"));
            }
            for (i, p) in page.items.iter().enumerate() {
                let text = if p.contents.is_some() {
                    p.to_string()
                } else {
                    format!("{p}  (not yours — read only)")
                };
                out.push(row_line(&text, i == *cursor));
            }
            out
        }
        PlaylistEdit::Name(text) => vec![
            Line::from(Span::styled("New playlist name", bold)),
            Line::from(format!("  {text}▏")),
            Line::from(""),
            hint("  enter creates it (private) · esc cancels"),
        ],
        PlaylistEdit::ConfirmRemove { playlist_name, .. } => vec![
            Line::from(Span::styled(
                format!("Remove this track from \"{playlist_name}\"?"),
                bold,
            )),
            Line::from(""),
            hint("  y removes it · n or esc keeps it"),
        ],
    }
}

// ---------------------------------------------------------------------------
// Liked (7.7)
// ---------------------------------------------------------------------------

fn liked_lines(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    let page = &app.web.liked;
    let mut out = Vec::new();
    // Whether the *playing* track is liked is not a row of this list, so it goes
    // above it the way the History tab's ▶ row does.
    if let Some((liked, now)) = liked_state(app) {
        if liked {
            out.push(Line::from(Span::styled(
                format!("  liked: {now}"),
                theme.accent_style().add_modifier(Modifier::BOLD),
            )));
        } else {
            out.push(hint(format!("  not liked: {now}")));
        }
    }
    if page.items.is_empty() {
        out.push(hint("  nothing liked yet"));
        out.extend(more(page));
        return out;
    }
    out.extend(page.items.iter().map(|item| fixed_line(&item.to_string())));
    out.extend(more(page));
    out
}

/// What the Liked tab can say about the track that is playing.
///
/// `None` when the check has not been made -- `f` is what makes it, and an
/// unasked question has no answer to print -- and `None` for an advert, which
/// has no track link, so there is nothing on screen for a like to be about.
fn liked_state(app: &App) -> Option<(bool, String)> {
    let liked = app.web.liked_here?;
    app.track().filter(|t| t.uri.is_some())?;
    Some((liked, now_playing(app)?))
}

// ---------------------------------------------------------------------------
// Queue (7.8)
// ---------------------------------------------------------------------------

/// Now playing, then what is after it.
///
/// When the Web API has nothing queued, the tab says why and then shows what trak
/// does know: the songs it has seen go by (owner, 2026-10-03 -- the tab was simply
/// empty for them). What it cannot show is the honest answer, "what Spotify will
/// play next": only Spotify knows that, and it only tells the Web API on Premium.
/// So the tab says that in a line and offers the history trak keeps itself, under
/// a heading that does not pretend to be a queue.
///
/// Nothing here warns a Premium user about a limit they cannot reach: it is the
/// add (`A`) that hits the 403, not this read.
fn queue_lines(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    let Queue {
        now_playing,
        upcoming,
    } = &app.web.queue;
    let mut out = Vec::new();
    match now_playing {
        Some(track) => out.push(Line::from(Span::styled(
            format!("\u{25b6} {track}"),
            theme.accent_style().add_modifier(Modifier::BOLD),
        ))),
        None => out.push(hint("  nothing is playing")),
    }
    if upcoming.is_empty() {
        out.push(hint(
            "  Spotify only tells the Web API what is queued on Premium, so trak cannot show what is next",
        ));
        // What trak does know. `history` is oldest-first, so it is walked
        // backwards for the History tab's newest-first order.
        let recent: Vec<String> = app
            .history
            .iter()
            .rev()
            .take(RECENT_ON_QUEUE)
            .map(|h| format!("   {} — {}", h.track.artist, h.track.title))
            .collect();
        if recent.is_empty() {
            out.push(hint("  and nothing has played yet this session"));
            return out;
        }
        out.push(Line::from(Span::styled("  Recently played", Theme::dim())));
        out.extend(recent.into_iter().map(|line| fixed_line(&line)));
        return out;
    }
    out.push(Line::from(Span::styled("  Up next", Theme::dim())));
    out.extend(upcoming.iter().map(|track| fixed_line(&track.to_string())));
    out
}

// ---------------------------------------------------------------------------
// Library (7.9)
// ---------------------------------------------------------------------------

/// One of the three sections, with the section strip above it.
///
/// The three lists load when they are first opened, so most visits to this tab
/// show one loaded list and two that have not been asked for. "Not loaded" and
/// "empty" are different facts about a person's library and only one of them is
/// a thing about them, so the two get different words.
fn library_lines(app: &App, theme: &Theme) -> Vec<Line<'static>> {
    let library = &app.web.library;
    let mut out = vec![section_strip(library.section, theme)];
    if !library.loaded[section_index(library.section)] {
        out.push(hint(
            "  not loaded yet — a list is fetched the first time you open it",
        ));
        return out;
    }
    let count = match library.section {
        LibrarySection::Albums => library.albums.items.len(),
        LibrarySection::Artists => library.artists.items.len(),
        LibrarySection::Recent => library.recent.items.len(),
    };
    if count == 0 {
        out.push(hint(format!("  {}", empty_section(library.section))));
        return out;
    }
    // The rows sit a cell further in than a list row, because the strip above
    // already indents: they belong to the named section rather than to the tab.
    match library.section {
        LibrarySection::Albums => {
            out.extend(
                library
                    .albums
                    .items
                    .iter()
                    .map(|a| fixed_line(&a.to_string())),
            );
            out.extend(more(&library.albums));
        }
        LibrarySection::Artists => {
            out.extend(
                library
                    .artists
                    .items
                    .iter()
                    .map(|a| fixed_line(&a.to_string())),
            );
            out.extend(more(&library.artists));
        }
        LibrarySection::Recent => {
            out.extend(
                library
                    .recent
                    .items
                    .iter()
                    .map(|t| fixed_line(&t.to_string())),
            );
            out.extend(more(&library.recent));
        }
    }
    out
}

/// The three section names with the one showing picked out, drawn the way the
/// tab strip picks out the selected tab: the same control, one level down.
fn section_strip(section: LibrarySection, theme: &Theme) -> Line<'static> {
    let mut spans = Vec::new();
    for one in LibrarySection::ALL {
        // Both forms are padded the same width, so the strip has the same shape
        // whichever section is showing and the selected one reads as a filled
        // block rather than a bracketed word in a gap.
        //
        // And the selected one is the album's gradient rather than one flat
        // accent, for the same reason the focused tab is (owner, 2026-10-03):
        // this strip is the only thing on the screen saying which of three lists
        // you are looking at, so it should look like the rest of the picture.
        let label = if one == section {
            format!(" [{}] ", one.label())
        } else {
            format!(" {} ", one.label())
        };
        if one == section {
            spans.extend(crate::tui::render::gradient_title(&label, theme));
        } else {
            spans.push(Span::styled(label, Style::default()));
        }
    }
    Line::from(spans)
}

/// Which of the three `loaded` flags belongs to a section. `ALL` is the index
/// order, so this cannot be wrong; the fallback is `Albums` rather than a panic
/// because a value outside the enum would mean the enum changed under us.
fn section_index(section: LibrarySection) -> usize {
    LibrarySection::ALL
        .iter()
        .position(|s| *s == section)
        .unwrap_or(0)
}

/// What a loaded section with nothing in it says. Never the same words as "not
/// loaded": an empty library is a fact about the library.
fn empty_section(section: LibrarySection) -> &'static str {
    match section {
        LibrarySection::Albums => "no saved albums",
        LibrarySection::Artists => "no followed artists",
        LibrarySection::Recent => "nothing played here yet",
    }
}

// ---------------------------------------------------------------------------
// The pages (7.10)
// ---------------------------------------------------------------------------

/// The open page: what it is, how to leave it, and its rows.
fn page_lines(app: &App, theme: &Theme, open: &Open) -> Vec<Line<'static>> {
    let mut out = vec![Line::from(Span::styled(
        format!("  {}", page_title(open)),
        theme.accent_style().add_modifier(Modifier::BOLD),
    ))];
    if matches!(open, Open::Artist(_)) {
        // `GET /artists/{id}/top-tracks` was removed in dev mode with no
        // replacement, so the page is albums and nothing else. Saying so is
        // better than a person hunting for a list that is never coming.
        out.push(hint(
            "  albums only — Spotify removed artist top tracks from the Web API",
        ));
    }
    // `esc` goes on the way in, not at the bottom of a long tracklist: a page
    // you cannot see your way out of is a trap, and `App` keeps a stack
    // precisely so `esc` from an album returns to the artist.
    out.push(hint("  esc  back"));
    let rows = app.web.open_rows();
    if rows.is_empty() {
        out.push(hint(format!("  {}", empty_page(open))));
        return out;
    }
    out.extend(
        rows.iter()
            .enumerate()
            .map(|(i, text)| row_line(text, i == app.web.open_cursor)),
    );
    out
}

/// Which kind of page this is. The state holds an id and no name for it, so the
/// heading can only be the kind -- and the rows underneath are what identify the
/// thing being looked at.
fn page_title(open: &Open) -> &'static str {
    match open {
        Open::Playlist(_) => "Playlist",
        Open::Artist(_) => "Artist",
        Open::Album(_) => "Album",
    }
}

fn empty_page(open: &Open) -> &'static str {
    match open {
        Open::Playlist(_) => "no tracks in this playlist",
        Open::Artist(_) => "no albums here",
        Open::Album(_) => "no tracks in this album",
    }
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

/// The track that is playing, as a row, and `None` when there is not one.
///
/// `artist — title`, the way the History tab's ▶ row writes it, including the
/// fallback for a track with no artist in it.
fn now_playing(app: &App) -> Option<String> {
    let track = app.track()?;
    Some(if track.artist.is_empty() {
        track.title.clone()
    } else {
        format!("{} — {}", track.artist, track.title)
    })
}

/// One list row, with the `›` the History tab uses, so a selected row looks the
/// same in every tab.
fn row_line(text: &str, selected: bool) -> Line<'static> {
    let (marker, style) = if selected {
        ("›", Style::default().add_modifier(Modifier::REVERSED))
    } else {
        (" ", Style::default())
    };
    Line::from(Span::styled(format!("{marker} {text}"), style))
}

/// A row in a list the user cannot move a cursor through -- the queue's up next
/// and the three Library sections, none of which has a cursor in the state.
///
/// A cell further in than [`row_line`], because these rows belong to the heading
/// above them rather than to the tab, and no `›`: a marker that never moves is
/// a selection the pane is pretending to have.
fn fixed_line(text: &str) -> Line<'static> {
    Line::from(Span::raw(format!("   {text}")))
}

/// A quiet line: an empty state, a hint, a status. Italic as well as dim
/// because "no results" and a row of results must never look alike.
///
/// The two-space indent is in the caller's string, not here, so that every
/// non-row line in a pane starts in the same column and one call site can say
/// "flush left" on purpose.
fn hint(text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(
        text.into(),
        Theme::dim().add_modifier(Modifier::ITALIC),
    ))
}

/// The line that says a list continues, and nothing at all when it does not.
///
/// `next: None` is the end of a list, not "not asked" (`web::api::Page`), so a
/// list without one must not hint that there is more to scroll to.
fn more<T>(page: &Page<T>) -> Option<Line<'static>> {
    page.next.as_ref().map(|_| hint("  more ↓"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::parse::parse;
    use crate::testutil::fixture;
    use crate::tui::app::{Connection, Event, update};
    use crate::web::api::{
        Album, Artist, FakeLibrary, Library, Playlist, PlaylistContents, Track, TrackItem,
    };

    // -- data ---------------------------------------------------------------

    fn artist(name: &str) -> Artist {
        Artist {
            id: format!("artist-{name}"),
            name: name.to_string(),
            uri: String::new(),
            images: Vec::new(),
        }
    }

    fn album(name: &str, year: &str) -> Album {
        Album {
            id: format!("album-{name}"),
            name: name.to_string(),
            uri: String::new(),
            release_date: Some(year.to_string()),
            artists: vec![artist("Jane Remover")],
            images: Vec::new(),
            total_tracks: Some(9),
        }
    }

    fn track(name: &str, by: &str) -> Track {
        Track {
            id: format!("track-{name}"),
            name: name.to_string(),
            uri: format!("spotify:track:{name}"),
            duration_ms: 201_000,
            track_number: Some(1),
            disc_number: Some(1),
            artists: vec![artist(by)],
            album: None,
        }
    }

    fn item(name: &str, by: &str) -> TrackItem {
        TrackItem {
            track: Some(track(name, by)),
        }
    }

    /// A playlist trak may read: `items` present, so `enter` opens it.
    fn readable_playlist(name: &str, total: u32) -> Playlist {
        Playlist {
            id: format!("playlist-{name}"),
            name: name.to_string(),
            uri: String::new(),
            description: None,
            images: Vec::new(),
            contents: Some(PlaylistContents {
                total: Some(total),
                items: Vec::new(),
            }),
        }
    }

    /// A playlist Spotify will not return items for: metadata only.
    fn opaque_playlist(name: &str) -> Playlist {
        Playlist {
            id: format!("playlist-{name}"),
            name: name.to_string(),
            uri: String::new(),
            description: None,
            images: Vec::new(),
            contents: None,
        }
    }

    /// An app with Spotify playing, so `app.track()` is `Some` where a test needs
    /// it. The fixture is real output from a real Spotify (docs/APPLESCRIPT.md).
    fn app_playing() -> App {
        update(
            App::new(),
            Event::PlayerState(Box::new(
                parse(&fixture("playing_track.txt")).expect("the playing fixture parses"),
            )),
        )
        .app
    }

    fn app_on(tab: Tab) -> App {
        let mut app = app_playing();
        app.tab = tab;
        app.web.connection = Connection::Connected;
        app
    }

    // -- reading what was drawn --------------------------------------------

    /// The lines as text. `lines` cannot know how tall or wide the pane is, so
    /// this is what the assertions that care about *content* read.
    fn shown(app: &App) -> String {
        lines(app, &Theme::default())
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The same lines drawn into a real buffer, which is the only way to see
    /// what a narrow or short pane does to them.
    fn painted(app: &App, w: u16, h: u16) -> String {
        let backend = ratatui::backend::TestBackend::new(w, h);
        let mut term =
            ratatui::Terminal::new(backend).expect("a TestBackend terminal can always be built");
        let drawn = lines(app, &Theme::default());
        term.draw(|f| {
            f.render_widget(
                ratatui::widgets::Paragraph::new(drawn),
                ratatui::layout::Rect::new(0, 0, w, h),
            )
        })
        .expect("drawing lines cannot fail");
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

    /// The pane as a buffer reads it, at a size every tab fits in: rows with
    /// their trailing blanks shaved off, and the empty rows below the last line
    /// dropped so the expectation is about what is written rather than about how
    /// tall the terminal is.
    fn painted_rows(app: &App, w: u16, h: u16) -> Vec<String> {
        let mut rows: Vec<String> = painted(app, w, h)
            .lines()
            .map(|row| row.trim_end().to_string())
            .collect();
        while rows.last().is_some_and(String::is_empty) {
            rows.pop();
        }
        rows
    }

    /// Assert what a tab reads as, twice: as the lines `lines` hands back, and
    /// as the cells a real `Paragraph` writes into an 80x24 buffer.
    ///
    /// The second is not the first again. A row too wide for the pane is dropped
    /// or wrapped there and nowhere else, a `Span` with no style is a row with no
    /// style, and a `TestBackend` is the only place any of that is visible.
    fn assert_paints(app: &App, want: &str) {
        // Trailing blanks are shaved off both sides: a cell that is a space is
        // indistinguishable from a cell that was never written, and asserting it
        // would pin the pane's padding rather than its text.
        let wanted: Vec<String> = want.lines().map(|row| row.trim_end().to_string()).collect();
        let shown_rows: Vec<String> = shown(app)
            .lines()
            .map(|r| r.trim_end().to_string())
            .collect();
        assert_eq!(shown_rows, wanted, "the lines `lines` returned");
        assert_eq!(painted_rows(app, 80, 24), wanted, "the pane at 80x24");
    }

    // -- 7.6 search ---------------------------------------------------------

    /// 7.6: the box, then the four groups as headings with their counts.
    #[test]
    fn the_search_tab_draws_the_box_and_the_four_groups() {
        let mut app = app_on(Tab::Search);
        app.web.query = "massive attack".into();
        app.web.search_shown = "massive attack".into();
        app.web.results = SearchResults {
            tracks: vec![track("Blue Lines", "Massive Attack")],
            albums: vec![album("Mezzanine", "1998")],
            artists: vec![artist("Massive Attack")],
            playlists: vec![readable_playlist("Blue Lines", 9)],
        };
        assert_paints(
            &app,
            concat!(
                "/ massive attack\n",
                "  Tracks (1)\n",
                "› Massive Attack — Blue Lines\n",
                "  Albums (1)\n",
                "› Mezzanine — Jane Remover (1998)\n",
                "  Artists (1)\n",
                "› Massive Attack\n",
                "  Playlists (1)\n",
                "› Blue Lines · 9 tracks",
            ),
        );
    }

    /// The caret is the only difference between focused and not, and a search
    /// that cannot be seen is a search nobody knows they are in.
    #[test]
    fn the_caret_is_there_only_while_the_box_has_focus() {
        let mut app = app_on(Tab::Search);
        app.web.query = "k".into();
        let unfocused = shown(&app);
        assert!(!unfocused.contains('▏'), "{unfocused}");

        app.web.search_focus = true;
        let focused = shown(&app);
        assert!(focused.contains('▏'), "{focused}");
        // The query is still all there: the caret is appended, not substituted.
        assert!(focused.contains("/ k"), "{focused}");
    }

    /// A group with nothing in it keeps its heading, dim, so the shape of the
    /// answer does not move as the query narrows.
    #[test]
    fn a_group_with_nothing_in_it_keeps_its_heading() {
        let mut app = app_on(Tab::Search);
        app.web.query = "nothing at all".into();
        app.web.search_shown = "nothing at all".into();
        app.web.results = SearchResults {
            tracks: vec![track("Blue Lines", "Massive Attack")],
            ..SearchResults::default()
        };
        let text = shown(&app);
        assert!(text.contains("Tracks (1)"), "{text}");
        for (name, count) in [("Albums", 0), ("Artists", 0), ("Playlists", 0)] {
            assert!(text.contains(&format!("{name} ({count})")), "{text}");
        }
        assert!(
            !text.contains("nothing matched"),
            "one track did match: {text}"
        );
    }

    /// A zero-count heading is `dim`, whatever the cursor is doing: a heading
    /// with nothing under it should recede rather than claim to be selected.
    #[test]
    fn a_zero_count_heading_is_dim_even_when_it_is_the_current_group() {
        let mut app = app_on(Tab::Search);
        app.web.query = "x".into();
        app.web.group = 2;
        let lines = lines(&app, &Theme::default());
        let heading = lines
            .iter()
            .find(|line| line.to_string().contains("Artists"))
            .expect("an Artists heading");
        let span = &heading.spans[0];
        assert_eq!(span.content.as_ref(), "  Artists (0)");
        assert_eq!(
            span.style,
            Theme::dim(),
            "an empty group must not look selected"
        );
    }

    /// The cursor is a group and a row within it, and the row it points at is
    /// the one that is marked.
    #[test]
    fn the_search_cursor_marks_the_row_within_its_group() {
        let mut app = app_on(Tab::Search);
        app.web.query = "x".into();
        app.web.results = SearchResults {
            albums: vec![album("Mezzanine", "1998"), album("Blue Lines", "1991")],
            ..SearchResults::default()
        };
        app.web.group = 1;
        app.web.group_row = [0, 1, 0, 0];
        let text = shown(&app);
        let albums: Vec<&str> = text.lines().skip(2).take(3).collect();
        assert_eq!(
            &albums[..2],
            &["  Albums (2)", "  Mezzanine — Jane Remover (1998)"],
            "{text}"
        );
        assert!(albums[2].starts_with('›'), "the second album: {text}");
    }

    /// 7.6: while a search is out, the line the results would go says so, and
    /// the results that are already on screen stay where they are.
    #[test]
    fn a_search_in_flight_says_so_where_the_results_go() {
        let mut app = app_on(Tab::Search);
        app.web.query = "mas".into();
        app.web.searching = true;
        let text = shown(&app);
        assert_eq!(text.lines().next(), Some("/ mas"), "{text}");
        assert_eq!(text.lines().nth(1), Some("  searching…"), "{text}");

        app.web.searching = false;
        app.web.search_shown = "ma".into();
        let settled = shown(&app);
        assert!(!settled.contains("searching…"), "{settled}");
        // And the groups are still there, one row further up.
        assert!(settled.contains("  Tracks (0)"), "{settled}");
    }

    #[test]
    fn an_unsearched_box_says_how_to_start_one() {
        let text = shown(&app_on(Tab::Search));
        assert!(text.contains("type to search"), "{text}");
        assert!(text.contains("/ focuses the box"), "{text}");
    }

    // -- 7.7 playlists ------------------------------------------------------

    #[test]
    fn the_playlists_tab_lists_them_with_the_cursor_on_one() {
        let mut app = app_on(Tab::Playlists);
        app.web.playlists = Page {
            items: vec![
                readable_playlist("Roadwork", 12),
                readable_playlist("Chill", 3),
            ],
            next: None,
        };
        app.web.playlist_cursor = 1;
        assert_paints(&app, "  Roadwork · 12 tracks\n› Chill · 3 tracks");
    }

    /// `items` is absent for a playlist trak may not read, so the row must not
    /// look like one it can open.
    #[test]
    fn a_playlist_trak_may_not_read_says_so() {
        let mut app = app_on(Tab::Playlists);
        app.web.playlists = Page {
            items: vec![
                opaque_playlist("Someone Else's"),
                readable_playlist("Mine", 1),
            ],
            next: None,
        };
        let text = shown(&app);
        assert!(text.contains("Someone Else's  (no tracklist)"), "{text}");
        assert!(
            text.contains("Mine · 1 tracks"),
            "and the other is not: {text}"
        );
        // The why, once, rather than on every row.
        assert_eq!(text.matches("collaborate").count(), 1, "{text}");
    }

    #[test]
    fn an_empty_playlists_tab_says_so() {
        assert!(shown(&app_on(Tab::Playlists)).contains("no playlists yet"));
    }

    // -- 7.7 liked -----------------------------------------------------------

    /// The like state of the *current* track, above a list it may not even be in.
    #[test]
    fn the_liked_tab_answers_whether_the_current_track_is_liked() {
        let mut app = app_on(Tab::Liked);
        app.web.liked = Page {
            items: vec![item("Nights", "Frank Ocean")],
            next: None,
        };
        app.web.liked_here = Some(true);
        assert_paints(
            &app,
            concat!(
                "  liked: Jane Remover — Census Designated\n",
                "   Frank Ocean — Nights",
            ),
        );

        app.web.liked_here = Some(false);
        let not = shown(&app);
        assert!(not.contains("not liked: "), "{not}");
        // The one truth both states share is the current track's name.
        assert!(not.contains("Jane Remover — Census Designated"), "{not}");
    }

    /// Unasked is not the same as "no": nothing is printed for a check that has
    /// not happened, so the line cannot be a stale answer to a later question.
    #[test]
    fn an_unchecked_like_state_prints_nothing() {
        let mut app = app_on(Tab::Liked);
        app.web.liked_here = None;
        app.web.liked = Page {
            items: vec![item("Nights", "Frank Ocean")],
            next: None,
        };
        let text = shown(&app);
        assert!(!text.contains("liked"), "{text}");
        assert!(!text.contains("not liked"), "{text}");
        assert!(text.contains("Frank Ocean — Nights"), "the list: {text}");
    }

    #[test]
    fn an_empty_liked_tab_says_so() {
        assert!(shown(&app_on(Tab::Liked)).contains("nothing liked yet"));
    }

    /// An advert is not a track: it has no URI, so there is nothing for a like to
    /// be about and a state printed beside it would be about a different song.
    #[test]
    fn an_advert_has_no_like_state_to_print() {
        let mut app = update(
            App::new(),
            Event::PlayerState(Box::new(
                parse(&fixture("playing_ad.txt")).expect("the advert fixture parses"),
            )),
        )
        .app;
        app.tab = Tab::Liked;
        app.web.connection = Connection::Connected;
        app.web.liked_here = Some(true);
        app.web.liked = Page {
            items: vec![item("Nights", "Frank Ocean")],
            next: None,
        };
        assert!(
            app.track()
                .expect("an advert is still something")
                .uri
                .is_none()
        );
        let text = shown(&app);
        assert!(!text.contains("liked: "), "{text}");
        assert!(text.contains("Frank Ocean — Nights"), "the list: {text}");
    }

    // -- 7.8 queue -----------------------------------------------------------

    /// 7.8: now playing, then up next. The read is not Premium-gated, so
    /// nothing here talks about Premium.
    #[test]
    fn the_queue_tab_is_now_playing_then_up_next() {
        let mut app = app_on(Tab::Queue);
        app.web.queue = Queue {
            now_playing: Some(track("Nights", "Frank Ocean")),
            upcoming: vec![
                track("Solo", "Frank Ocean"),
                track("Self Control", "Frank Ocean"),
            ],
        };
        assert_paints(
            &app,
            concat!(
                "▶ Frank Ocean — Nights\n",
                "  Up next\n",
                "   Frank Ocean — Solo\n",
                "   Frank Ocean — Self Control",
            ),
        );
        assert!(!shown(&app).to_lowercase().contains("premium"));
    }

    /// An empty Web API queue is the Free-tier case, so it explains itself and
    /// shows what trak does know instead of being an empty page (owner,
    /// 2026-10-03).
    #[test]
    fn an_empty_queue_explains_itself_and_shows_what_was_played() {
        let text = shown(&app_on(Tab::Queue));
        assert!(text.contains("nothing is playing"), "{text}");
        assert!(text.contains("Premium"), "{text}");
        assert!(
            !text.contains("nothing played yet"),
            "an empty session says so: {text}"
        );

        let mut app = app_on(Tab::Queue);
        app.web.queue.now_playing = Some(track("Nights", "Frank Ocean"));
        for (title, artist) in [("Solo", "Frank Ocean"), ("Self Control", "Frank Ocean")] {
            app.history.push(crate::tui::app::HistoryEntry {
                track: crate::player::TrackInfo {
                    title: title.into(),
                    artist: artist.into(),
                    ..crate::player::fake::sample_track()
                },
                at: std::time::Instant::now(),
            });
        }
        let text = shown(&app);
        assert!(text.contains("Recently played"), "{text}");
        assert!(text.contains("Frank Ocean — Self Control"), "{text}");
        // Newest first, like the History tab.
        let newer = text.find("Self Control").unwrap();
        let older = text.find("— Solo").unwrap();
        assert!(newer < older, "newest first: {text}");
    }

    /// The history is capped, so the fallback cannot grow without bound either.
    #[test]
    fn the_queue_fallback_is_capped() {
        let mut app = app_on(Tab::Queue);
        for _ in 0..(RECENT_ON_QUEUE + 20) {
            app.history.push(crate::tui::app::HistoryEntry {
                track: crate::player::fake::sample_track(),
                at: std::time::Instant::now(),
            });
        }
        let rows = shown(&app)
            .lines()
            .filter(|l| l.contains("Census Designated"))
            .count();
        assert_eq!(rows, RECENT_ON_QUEUE, "capped at {RECENT_ON_QUEUE}");
    }

    // -- 7.9 library ---------------------------------------------------------

    /// 7.9: the strip names the three sections and picks the one showing, then
    /// the list itself.
    #[test]
    fn the_library_tab_draws_the_section_it_is_showing() {
        let mut app = app_on(Tab::Library);
        app.web.library.section = LibrarySection::Albums;
        app.web.library.loaded = [true, true, true];
        app.web.library.albums = Page {
            items: vec![album("Census", "2022"), album("Blue Lines", "1991")],
            next: None,
        };
        app.web.library.artists = Page {
            items: vec![artist("Jane Remover")],
            next: None,
        };
        app.web.library.recent = Page {
            items: vec![track("Census Designated", "Jane Remover")],
            next: None,
        };
        assert_paints(
            &app,
            concat!(
                " [Saved albums]  Followed artists  Recently played \n",
                "   Census — Jane Remover (2022)\n",
                "   Blue Lines — Jane Remover (1991)",
            ),
        );

        app.web.library.section = LibrarySection::Artists;
        assert_paints(
            &app,
            concat!(
                " Saved albums  [Followed artists]  Recently played \n",
                "   Jane Remover",
            ),
        );
        app.web.library.section = LibrarySection::Recent;
        assert_paints(
            &app,
            concat!(
                " Saved albums  Followed artists  [Recently played] \n",
                "   Jane Remover — Census Designated",
            ),
        );
    }

    /// "Not loaded" and "empty" are different facts, and a tab that cannot tell
    /// them apart tells the user their library is empty when trak has not looked.
    #[test]
    fn a_section_that_has_not_loaded_is_not_an_empty_one() {
        let mut app = app_on(Tab::Library);
        app.web.library.section = LibrarySection::Artists;
        let unloaded = shown(&app);
        assert!(unloaded.contains("not loaded yet"), "{unloaded}");

        app.web.library.loaded = [false, true, false];
        let empty = shown(&app);
        assert!(!empty.contains("not loaded yet"), "{empty}");
        assert!(empty.contains("no followed artists"), "{empty}");
        assert_ne!(unloaded, empty);
    }

    #[test]
    fn each_empty_section_says_something_of_its_own() {
        let mut app = app_on(Tab::Library);
        app.web.library.loaded = [true; 3];
        let mut said = Vec::new();
        for section in LibrarySection::ALL {
            app.web.library.section = section;
            let text = shown(&app);
            let line = text.lines().nth(1).unwrap_or_default().trim().to_string();
            said.push(line.clone());
            assert!(!line.is_empty(), "{section:?} said nothing: {text}");
            assert!(!line.contains("not loaded"), "{section:?}: {text}");
        }
        // Three sections, three different empty states: a shared "nothing here"
        // would not say which list was empty.
        let unique: std::collections::HashSet<&String> = said.iter().collect();
        assert_eq!(unique.len(), said.len(), "{said:?}");
    }

    // -- 7.10 pages ----------------------------------------------------------

    /// 7.10: the page says what it is, and `esc` is on the way in rather than at
    /// the bottom of a long tracklist.
    #[test]
    fn each_page_says_which_page_it_is_and_how_to_leave_it() {
        let mut app = app_on(Tab::Playlists);
        app.web.open_playlist("playlist-1".into());
        app.web.playlist_items(
            "playlist-1".into(),
            vec![
                item("Blue Lines", "Massive Attack"),
                item("Unfinished", "Massive Attack"),
            ],
        );
        app.web.open_cursor = 1;
        // `esc` comes before the rows, so it is on screen before it is needed: a
        // page you cannot see your way out of is a trap.
        assert_paints(
            &app,
            concat!(
                "  Playlist\n",
                "  esc  back\n",
                "  Massive Attack — Blue Lines\n",
                "› Massive Attack — Unfinished",
            ),
        );

        app.web.open_artist("artist-1".into());
        app.web
            .artist_albums("artist-1".into(), vec![album("Mezzanine", "1998")]);
        // Top tracks were removed from the API, so the page says so rather than
        // leaving a person looking for a list that cannot arrive.
        assert_paints(
            &app,
            concat!(
                "  Artist\n",
                "  albums only — Spotify removed artist top tracks from the Web API\n",
                "  esc  back\n",
                "› Mezzanine — Jane Remover (1998)",
            ),
        );

        app.web.open_album("album-1".into());
        app.web.album_tracks(
            "album-1".into(),
            vec![track("Blue Lines", "Massive Attack")],
        );
        assert_paints(
            &app,
            concat!(
                "  Album\n",
                "  esc  back\n",
                "› Massive Attack — Blue Lines",
            ),
        );
    }

    /// A page replaces the list it was opened from: two lists on one pane is a
    /// pane where the key does something you cannot see.
    #[test]
    fn an_open_page_hides_the_list_it_came_from() {
        let mut app = app_on(Tab::Playlists);
        app.web.playlists = Page {
            items: vec![readable_playlist("Roadwork", 12)],
            next: None,
        };
        assert!(shown(&app).contains("Roadwork"), "before");
        app.web.open_playlist("playlist-Roadwork".into());
        let text = shown(&app);
        assert!(!text.contains("Roadwork · 12 tracks"), "{text}");
        assert!(text.contains("Playlist"), "{text}");
    }

    // -- the connection, which is the point of the whole module -------------

    /// Every state that is not connected says what to do, on the first line, in
    /// every tab -- a person with no Client ID cannot be left looking at a pane
    /// that might as well be broken.
    #[test]
    fn an_unconnected_client_is_told_in_every_tab() {
        for tab in Tab::VERSION_A {
            for (connection, want) in [
                (Connection::NoClientId, "press , then s"),
                (Connection::LoggedOut, "press , then s"),
                (Connection::NeedsRelogin, "press , then s"),
            ] {
                let mut app = app_on(tab);
                app.web.connection = connection;
                let text = shown(&app);
                assert!(
                    text.lines().next().unwrap_or_default().contains(want),
                    "{tab:?} as {connection:?} said nothing first: {text}"
                );
            }
        }
    }

    /// And the other direction: a connected client must stop talking about
    /// connecting. A tab that keeps explaining itself is worse than one that
    /// never did.
    #[test]
    fn a_connected_client_is_told_nothing() {
        for tab in Tab::VERSION_A {
            let text = shown(&app_on(tab));
            assert!(Connection::Connected.notice().is_none());
            for word in ["trak config", "connect", "reconnect", "Client ID"] {
                assert!(!text.contains(word), "{tab:?} still says {word}: {text}");
            }
        }
    }

    // -- paging --------------------------------------------------------------

    /// `next: Some` is a longer list and says so; `next: None` is the end and
    /// must not hint at anything below it. The fake is the only way to hold a
    /// real `Continuation`, and it makes one whenever a list is longer than a
    /// page.
    #[test]
    fn a_list_with_more_looks_different_at_the_bottom() {
        // The fake keeps the liked list as URIs and resolves it against the
        // catalogue, so both have to be seeded for the list to be 21 long.
        let many: Vec<Track> = (0..21).map(|i| track(&format!("t{i}"), "Jane")).collect();
        let library = FakeLibrary::with_catalogue(many.clone(), Vec::new(), Vec::new(), Vec::new())
            .with_library(many, Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let paged = library.liked_tracks(None).expect("the fake answers");
        assert!(paged.next.is_some(), "21 items is more than one page");

        let mut app = app_on(Tab::Liked);
        app.web.liked = paged;
        let with_more = shown(&app);
        assert!(with_more.ends_with("  more ↓"), "{with_more}");

        app.web.liked.next = None;
        let at_the_end = shown(&app);
        assert!(!at_the_end.contains("more"), "{at_the_end}");
        assert_ne!(with_more, at_the_end);
        // The line is at the bottom, and it is the only difference.
        assert_eq!(with_more.lines().count(), at_the_end.lines().count() + 1);
    }

    // -- the invariants that keep it from taking the TUI down ---------------

    /// The sizes every tab has to survive. Narrower than 30 is `TooSmall` for the
    /// whole dashboard, but a tab pane can still be handed a strip this short by
    /// a resize, and a row that is two cells too wide is a torn border.
    const SIZES: [(u16, u16); 4] = [(10, 3), (20, 8), (40, 24), (80, 24)];

    /// A frame is whole when every row is exactly the pane's width and there are
    /// exactly the pane's rows: `Paragraph` clips rather than wrapping, and this
    /// is what says it did.
    fn assert_whole(screen: &str, what: &str, w: u16, h: u16) {
        let rows: Vec<&str> = screen.lines().collect();
        assert_eq!(
            rows.len(),
            h as usize,
            "{what} at {w}x{h} has the wrong rows"
        );
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(
                row.chars().count(),
                w as usize,
                "{what} at {w}x{h} tore row {i}: {row:?}"
            );
        }
    }

    /// Every tab with every list full, at every size. `lines` hands back more
    /// lines than any pane has rows and never learns the width, so this is the
    /// only check that a narrow or short pane clips rather than panics.
    #[test]
    fn no_tab_panics_at_any_size() {
        for tab in Tab::VERSION_A {
            for (w, h) in SIZES {
                let app = every_state_on(tab);
                assert_whole(&painted(&app, w, h), &format!("{tab:?}"), w, h);
            }
        }
    }

    /// And each kind of page, which replaces the list rather than joining it. A
    /// sweep over the lists alone would never draw one of these at all.
    #[test]
    fn no_page_panics_at_any_size() {
        for (name, app) in pages() {
            for (w, h) in SIZES {
                assert_whole(&painted(&app, w, h), &name, w, h);
            }
        }
    }

    /// A cursor or a count from outside every list must not make the pane say
    /// less than it can, and must certainly not index off the end.
    #[test]
    fn out_of_range_state_renders_instead_of_panicking() {
        for tab in Tab::VERSION_A {
            let mut app = every_state_on(tab);
            app.web.group = 99;
            app.web.group_row = [99, 99, 99, 99];
            app.web.playlist_cursor = 99;
            for (w, h) in SIZES {
                assert_whole(&painted(&app, w, h), &format!("{tab:?}"), w, h);
            }
            // The four headings are all still there, whatever the cursor says.
            if tab == Tab::Search {
                for name in GROUPS {
                    assert!(shown(&app).contains(&format!("{name} (")), "{app:?}");
                }
            }
        }
        for (name, mut app) in pages() {
            app.web.open_cursor = 99;
            for (w, h) in SIZES {
                assert_whole(&painted(&app, w, h), &name, w, h);
            }
        }
    }

    /// A tab that is not a Web API tab must still produce lines rather than an
    /// empty pane, since `render.rs` routes whatever it does not know to here.
    #[test]
    fn a_tab_with_no_web_side_says_so() {
        for tab in [Tab::History, Tab::Info, Tab::Lyrics] {
            let text = shown(&app_on(tab));
            assert!(
                text.contains("does not use the Spotify Web API"),
                "{tab:?}: {text}"
            );
        }
    }

    /// `Page` and `SearchResults` are what the API client is tested against, so
    /// a row in a tab has to be the same string the client would print.
    #[test]
    fn rows_are_the_display_impls_not_our_own_formatting() {
        let mut app = app_on(Tab::Search);
        let one = album("Mezzanine", "1998-01-20");
        app.web.results = SearchResults {
            albums: vec![one.clone()],
            ..SearchResults::default()
        };
        assert!(
            shown(&app).contains(&one.to_string()),
            "the tab and the Display impl have drifted: {}",
            one
        );

        app.web.results = SearchResults {
            playlists: vec![readable_playlist("Roadwork", 12)],
            ..SearchResults::default()
        };
        assert!(shown(&app).contains("Roadwork · 12 tracks"));
    }

    /// An app with every list full and every section loaded, for the size sweeps
    /// and the out-of-range sweep to bite on something real.
    fn every_state_on(tab: Tab) -> App {
        let mut app = app_on(tab);
        app.web.results = SearchResults {
            tracks: vec![track("Blue Lines", "Massive Attack")],
            albums: vec![album("Mezzanine", "1998")],
            artists: vec![artist("Massive Attack")],
            playlists: vec![readable_playlist("Blue Lines", 9)],
        };
        app.web.query = "massive attack".into();
        app.web.search_focus = true;
        app.web.playlists = Page {
            items: vec![
                readable_playlist("Roadwork", 12),
                opaque_playlist("Not Mine"),
            ],
            next: None,
        };
        app.web.liked = Page {
            items: vec![item("Nights", "Frank Ocean")],
            next: None,
        };
        app.web.liked_here = Some(true);
        app.web.queue = Queue {
            now_playing: Some(track("Nights", "Frank Ocean")),
            upcoming: vec![track("Solo", "Frank Ocean")],
        };
        app.web.library.loaded = [true; 3];
        app.web.library.albums = Page {
            items: vec![album("Census", "2022")],
            next: None,
        };
        app.web.library.artists = Page {
            items: vec![artist("Jane Remover")],
            next: None,
        };
        app.web.library.recent = Page {
            items: vec![track("Census Designated", "Jane Remover")],
            next: None,
        };
        app
    }

    /// The three kinds of page, each opened over a full app so the sweep has
    /// rows to clip.
    fn pages() -> Vec<(String, App)> {
        let mut playlist = every_state_on(Tab::Playlists);
        playlist.web.open_playlist("playlist-1".into());
        playlist.web.playlist_items(
            "playlist-1".into(),
            vec![item("Blue Lines", "Massive Attack")],
        );
        let mut artist = every_state_on(Tab::Playlists);
        artist.web.open_artist("artist-1".into());
        artist
            .web
            .artist_albums("artist-1".into(), vec![album("Mezzanine", "1998")]);
        let mut album = every_state_on(Tab::Playlists);
        album.web.open_album("album-1".into());
        album.web.album_tracks(
            "album-1".into(),
            vec![track("Blue Lines", "Massive Attack")],
        );
        vec![
            ("a playlist page".to_string(), playlist),
            ("an artist page".to_string(), artist),
            ("an album page".to_string(), album),
        ]
    }

    /// 7.11: the modal replaces the tab body and says what each key does.
    #[test]
    fn the_playlist_modal_says_what_it_is_asking() {
        let mut app = app_on(Tab::Liked);
        app.web.playlists = Page {
            items: vec![],
            next: None,
        };
        app.web.edit = Some(PlaylistEdit::Pick {
            uri: "u".into(),
            cursor: 0,
        });
        let text = shown(&app);
        assert!(
            text.contains("to which playlist?") && text.contains("n makes a new"),
            "{text}"
        );
        assert!(text.contains("loading your playlists"), "{text}");

        app.web.edit = Some(PlaylistEdit::Name("Road".into()));
        let text = shown(&app);
        assert!(
            text.contains("Road▏") && text.contains("esc cancels"),
            "{text}"
        );

        app.web.edit = Some(PlaylistEdit::ConfirmRemove {
            playlist: "p".into(),
            playlist_name: "Gym".into(),
            uri: "u".into(),
        });
        let text = shown(&app);
        assert!(
            text.contains("Remove this track from \"Gym\"?") && text.contains("y removes"),
            "{text}"
        );
    }
}
