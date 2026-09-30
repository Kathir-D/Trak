// TODO 6.1: does LRCLIB actually answer for the track that is playing?
//
// `cargo run --release --example lyrics-probe -- "Artist" "Title" [seconds]`
// prints the result or the reason, so "the lyrics tab says nothing" can be told
// apart from "this song is not in the database" without a debugger.
fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(artist), Some(title)) = (args.next(), args.next()) else {
        eprintln!("usage: lyrics-probe <artist> <title> [duration-secs]");
        return;
    };
    let duration: Option<u64> = args.next().and_then(|d| d.parse().ok());
    let started = std::time::Instant::now();
    match trak::lyrics::fetch(&title, &artist, None, duration) {
        Ok(l) => {
            println!(
                "{artist} - {title}\n  {} lines, synced: {}, instrumental: {}\n  took {:?}",
                l.lines.len(),
                l.synced,
                l.instrumental,
                started.elapsed()
            );
            for line in l.lines.iter().take(6) {
                println!("    {:>7.2}  {}", line.time_secs, line.text);
            }
        }
        Err(e) => println!("{artist} - {title}\n  {e}\n  took {:?}", started.elapsed()),
    }
}
