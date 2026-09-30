//! trak — an interactive terminal UI for the Spotify desktop app on macOS.
//!
//! This is the bare scaffold (TODO.md task 0.x). It only answers `--version`
//! so CI and packaging have something real to build; everything else is
//! tracked in TODO.md.

fn main() {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("--version") | Some("-V") => println!("trak {}", env!("CARGO_PKG_VERSION")),
        _ => {
            eprintln!(
                "trak {} — not implemented yet, see TODO.md",
                env!("CARGO_PKG_VERSION")
            );
            std::process::exit(2);
        }
    }
}
