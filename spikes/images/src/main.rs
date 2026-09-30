// TODO 1.4: which image protocols actually work in cmux and Terminal.app?
//
// Builds one `ratatui_image::Protocol` per protocol type, renders all of them
// side by side, and holds the screen open so a `screencapture` of a real
// terminal window answers "terminal x protocol = works?" for every combination
// at once. The result is also appended to /tmp/spike/images.log, because an
// agent session cannot read the terminal it spawns -- it can only screenshot it.

use std::io::Write;

use image::RgbImage;
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{Image, Resize};

const OUT: &str = "/tmp/spike/images.log";

/// Only used if the terminal cannot be queried, so an image is still drawn.
const FONT_SIZE: (u16, u16) = (8, 16);

fn note(line: impl AsRef<str>) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(OUT)
    {
        writeln!(f, "{}", line.as_ref()).ok();
        let _ = f.flush();
    }
}

/// Four colour quadrants, a 32px grid and a white disc, so resampling artefacts
/// and wrong aspect ratios are obvious even at 20x10 cells.
fn test_image() -> image::DynamicImage {
    const W: u32 = 256;
    const H: u32 = 256;
    let mut img = RgbImage::new(W, H);
    for y in 0..H {
        for x in 0..W {
            let (qx, qy) = (x * 2 / W, y * 2 / H);
            let mut px = match (qy, qx) {
                (0, 0) => image::Rgb([220, 40, 40]),
                (0, 1) => image::Rgb([40, 200, 90]),
                (1, 0) => image::Rgb([50, 110, 240]),
                _ => image::Rgb([240, 200, 40]),
            };
            if x % 32 == 0 || y % 32 == 0 {
                px = image::Rgb([0, 0, 0]);
            }
            let dx = x as f32 - W as f32 / 2.0;
            let dy = y as f32 - H as f32 / 2.0;
            if (dx * dx + dy * dy).sqrt() < 60.0 {
                px = image::Rgb([255, 255, 255]);
            }
            img.put_pixel(x, y, px);
        }
    }
    image::DynamicImage::ImageRgb8(img)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = std::fs::remove_file(OUT);
    for v in [
        "TERM",
        "TERM_PROGRAM",
        "TERM_PROGRAM_VERSION",
        "COLORTERM",
        "LC_TERMINAL",
        "KITTY_WINDOW_ID",
        "TMUX",
        "STY",
        "WT_SESSION",
    ] {
        note(format!("ENV {v}={:?}", std::env::var(v)));
    }

    let img = test_image();

    // (1) What trak actually does at runtime: ask the terminal.
    match Picker::from_query_stdio() {
        Ok(p) => {
            note(format!(
                "AUTO ok protocol_type={:?} font_size={:?}",
                p.protocol_type(),
                p.font_size()
            ));
            for c in p.capabilities() {
                note(format!("AUTO cap {c:?}"));
            }
        }
        Err(e) => note(format!("AUTO failed: {e}")),
    }

    // (2) Force each protocol, so one screenshot covers all of them.
    let area = ratatui::layout::Rect::new(0, 0, 30, 16);
    let mut cells: Vec<(&str, Option<Protocol>)> = Vec::new();
    for (name, ty) in [
        ("halfblocks", ProtocolType::Halfblocks),
        ("kitty", ProtocolType::Kitty),
        ("iterm2", ProtocolType::Iterm2),
        ("sixel", ProtocolType::Sixel),
    ] {
        let mut p = Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize(FONT_SIZE));
        p.set_protocol_type(ty);
        match p.new_protocol(img.clone(), area, Resize::Fit(None)) {
            Ok(_) => note(format!("EXPLICIT {name}: ok")),
            Err(e) => note(format!("EXPLICIT {name}: FAILED {e}")),
        }
        cells.push((
            name,
            p.new_protocol(img.clone(), area, Resize::Fit(None)).ok(),
        ));
    }
    note("---- summary ----");
    for (n, p) in &cells {
        note(format!(
            "SUMMARY {n} = {}",
            if p.is_some() { "ok" } else { "FAILED" }
        ));
    }

    enable_raw_mode()?;
    let mut out = std::io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(out))?;
    term.clear()?;

    // drain anything already typed, so a stale keystroke cannot end the run early
    while event::poll(std::time::Duration::from_millis(50))? {
        let _ = event::read();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let size = term.backend().size()?;
        let area = ratatui::layout::Rect::new(0, 0, size.width * 2, size.height * 4);
        // build the protocols at the real on-screen size, not a guessed one
        let rebuilt: Vec<(&str, Option<Protocol>)> = cells
            .iter()
            .map(|(n, _)| {
                let ty = match *n {
                    "halfblocks" => ProtocolType::Halfblocks,
                    "kitty" => ProtocolType::Kitty,
                    "iterm2" => ProtocolType::Iterm2,
                    _ => ProtocolType::Sixel,
                };
                let mut p =
                    Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize(FONT_SIZE));
                p.set_protocol_type(ty);
                (
                    *n,
                    p.new_protocol(img.clone(), area, Resize::Fit(None)).ok(),
                )
            })
            .collect();
        term.draw(|f| draw(f, &rebuilt))?;
        if event::poll(std::time::Duration::from_millis(500))? {
            let _ = event::read();
        }
        if std::time::Instant::now() > deadline {
            break;
        }
    }

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    note("DONE");
    Ok(())
}

fn draw(f: &mut Frame, cells: &[(&str, Option<Protocol>)]) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(f.area());

    let mut header = vec![Span::styled(
        "TODO 1.4  --  press any key to quit",
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Cyan),
    )];
    for (name, p) in cells {
        let ok = p.is_some();
        header.push(Span::raw("   "));
        header.push(Span::styled(
            format!("{}={}", name, if ok { "ok" } else { "FAIL" }),
            Style::default().fg(if ok { Color::Green } else { Color::Red }),
        ));
    }
    f.render_widget(Line::from(header), rows[0]);

    let cols = Layout::horizontal(vec![Constraint::Ratio(1, cells.len() as u32); cells.len()])
        .split(rows[1]);
    for (i, (_, p)) in cells.iter().enumerate() {
        match p {
            Some(proto) => f.render_widget(Image::new(proto), cols[i]),
            None => f.render_widget(
                Line::from(Span::styled("unavailable", Style::default().fg(Color::Red))),
                cols[i],
            ),
        }
    }
}
