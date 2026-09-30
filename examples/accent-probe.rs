// TODO 4.2: what colour does a given cover actually produce?
//
// `cargo run --release --example accent-probe <image>...` prints the accent each
// image yields, and why when there is none. Useful when a cover "does not get a
// colour" and the question is whether that is the filter working or a bug.
fn main() {
    let mut any = false;
    for arg in std::env::args().skip(1) {
        let path = std::path::Path::new(&arg);
        any = true;
        match trak::art::decode(path) {
            Ok(img) => {
                let found = trak::accent::dominant_colour(&img);
                let fixed = found.map(trak::accent::ensure_contrast);
                println!(
                    "{arg}\n  size      {}x{}\n  dominant  {found:?}\n  contrast  {fixed:?}\n  usable    {}\n  luma      {:.3}",
                    img.width(),
                    img.height(),
                    fixed.is_some_and(trak::accent::is_usable),
                    match fixed {
                        Some(ratatui::style::Color::Rgb(r, g, b)) => trak::accent::luma(r, g, b),
                        _ => 0.0,
                    },
                );
            }
            Err(e) => println!("{arg}\n  could not decode: {e}"),
        }
    }
    if !any {
        let dir = trak::art::cache_dir();
        println!(
            "no arguments: reading every cached cover in {}",
            dir.display()
        );
        let entries = std::fs::read_dir(&dir).into_iter().flatten().flatten();
        for entry in entries {
            let path = entry.path();
            if let Ok(img) = trak::art::decode(&path) {
                let found = trak::accent::dominant_colour(&img);
                println!(
                    "{}\n  {}x{}  dominant {found:?}  usable {}",
                    path.display(),
                    img.width(),
                    img.height(),
                    found.is_some_and(trak::accent::is_usable),
                );
            }
        }
    }
}
