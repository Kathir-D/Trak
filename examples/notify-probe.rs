//! Measure the notification's arrival latency, which is TODO 3.9's "done when".
//!
//! Subscribes like the TUI does, then triggers a Spotify change from a second
//! process and prints how long the event took to arrive.
//!
//!   cargo run --release --example notify-probe

use std::time::{Duration, Instant};

use trak::player::notify;

fn osascript(script: &str) {
    let osa = std::env::var("TRAK_OSASCRIPT").unwrap_or_else(|_| "/usr/bin/osascript".into());
    let _ = std::process::Command::new(osa)
        .arg("-e")
        .arg(script)
        .output();
}

fn main() {
    let Some(sub) = notify::subscribe() else {
        eprintln!("could not subscribe; trak would fall back to the poll alone");
        std::process::exit(1);
    };
    eprintln!("subscribed; waiting for a Spotify change…");

    // Give the observer a moment to register on its run loop.
    std::thread::sleep(Duration::from_millis(300));

    let cases: [(&str, &str); 3] = [
        ("pause", r#"tell application "Spotify" to pause"#),
        ("play", r#"tell application "Spotify" to play"#),
        ("next track", r#"tell application "Spotify" to next track"#),
    ];

    let mut seen = 0;
    let deadline = Instant::now() + Duration::from_secs(60);
    for (label, script) in cases {
        let t0 = Instant::now();
        osascript(script);
        loop {
            match sub.poll() {
                Some(e) => {
                    println!(
                        "{label:12} -> event after {:>6.1}ms  state={:?} title={:?}",
                        t0.elapsed().as_secs_f64() * 1000.0,
                        e.playback,
                        e.title
                    );
                    seen += 1;
                    break;
                }
                None => {
                    if t0.elapsed() > Duration::from_secs(10) {
                        println!("{label:12} -> NO EVENT within 10s");
                        break;
                    }
                    // The wait *is* a run-loop pump. This is what the TUI does
                    // between frames, and without it nothing is ever delivered:
                    // NSDistributedNotificationCenter only delivers on the main
                    // run loop, whichever thread registered the observer.
                    notify::pump_run_loop(0.005);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(300));
        if Instant::now() > deadline {
            break;
        }
    }
    println!("\n{seen} of {} cases produced an event", cases.len());

    // And prove the asymmetry 1.3 recorded: a seek fires nothing.
    let t0 = Instant::now();
    osascript(r#"tell application "Spotify" to set player position to 30"#);
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        notify::pump_run_loop(0.02);
    }
    match sub.poll() {
        Some(_) => {
            println!("seek        -> FIRED an event (contradicts docs/APPLESCRIPT.md section 9)")
        }
        None => println!(
            "seek        -> no event, as documented ({:.0}ms waited)",
            t0.elapsed().as_secs_f64() * 1000.0
        ),
    }
}
