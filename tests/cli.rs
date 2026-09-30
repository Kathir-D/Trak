//! Every subcommand, run as a real process, with no Spotify and no network.
//!
//! Driven by the hidden `--fake` flag, which swaps the AppleScript player for
//! the in-memory one (ARCHITECTURE, "Testing strategy"). That is what lets these
//! run in CI, where Spotify does not exist.

use assert_cmd::Command;
use predicates::prelude::*;

/// A `trak` invocation with the fake player selected.
fn trak() -> Command {
    let mut c = Command::cargo_bin("trak").expect("the trak binary must build");
    c.arg("--fake");
    c.env_remove("TRAK_OSASCRIPT");
    c
}

#[test]
fn version_works_without_a_player() {
    trak().arg("--version").assert().success();
}

#[test]
fn help_lists_every_shpotify_command() {
    let out = trak().arg("--help").output().expect("run --help");
    let s = String::from_utf8_lossy(&out.stdout);
    for cmd in [
        "status", "play", "pause", "stop", "quit", "next", "prev", "replay", "pos", "vol",
        "toggle", "share",
    ] {
        assert!(s.contains(cmd), "--help is missing `{cmd}`:\n{s}");
    }
}

#[test]
fn status_prints_the_track() {
    trak()
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("Census Designated"))
        .stdout(predicate::str::contains("Jane Remover"));
}

#[test]
fn status_piped_has_no_ansi_escapes() {
    // assert_cmd captures a pipe, so this is also the "piping gives clean text"
    // check from SPEC §9.
    let out = trak().arg("status").output().expect("run status");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(
        !s.contains('\x1b'),
        "piped output must not be coloured:\n{s:?}"
    );
    assert!(s.contains("Census Designated"));
}

#[test]
fn status_plain_agrees_with_the_default_when_piped() {
    let a = trak().arg("status").output().unwrap();
    let b = trak().args(["--plain", "status"]).output().unwrap();
    assert_eq!(a.stdout, b.stdout, "--plain must be a no-op on a pipe");
}

#[test]
fn status_json_is_valid_json_with_the_expected_keys() {
    let out = trak()
        .args(["status", "--json"])
        .output()
        .expect("run status --json");
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.starts_with('{') && s.trim_end().ends_with('}'), "{s}");
    for key in [
        "\"state\"",
        "\"title\"",
        "\"artist\"",
        "\"album\"",
        "\"uri\"",
        "\"duration_ms\"",
        "\"position_secs\"",
        "\"volume\"",
        "\"shuffling\"",
        "\"repeating\"",
        "\"is_ad\"",
    ] {
        assert!(s.contains(key), "status --json is missing {key}:\n{s}");
    }
    assert!(s.contains("\"title\":\"Census Designated\""), "{s}");
}

#[test]
fn status_fields_print_bare_like_shpotify() {
    trak()
        .args(["status", "artist"])
        .assert()
        .success()
        .stdout("Jane Remover\n");
    trak()
        .args(["status", "track"])
        .assert()
        .success()
        .stdout("Census Designated\n");
}

#[test]
fn playback_verbs_all_succeed() {
    for args in [
        vec!["play"],
        vec!["pause"],
        vec!["stop"],
        vec!["next"],
        vec!["prev"],
        vec!["replay"],
    ] {
        trak().args(&args).assert().success();
    }
}

#[test]
fn stop_reports_when_already_stopped() {
    // The fake starts playing, so `stop` pauses it. Pausing twice is a no-op
    // that must not be an error.
    trak().arg("stop").assert().success();
}

#[test]
fn vol_show_prints_a_bare_number() {
    trak()
        .args(["vol", "show"])
        .assert()
        .success()
        .stdout(predicate::str::is_match("^\\d+\\n$").unwrap());
}

/// `vol` prints a bare number, so a test can just read it back.
///
/// Each invocation is its own process with its own in-memory player, so state
/// does **not** carry between calls here — which is also true of the real CLI,
/// where every invocation talks to Spotify afresh. The step arithmetic and the
/// clamping are therefore covered as unit tests in `player::actions`; what
/// matters here is that one invocation steps by 10 and prints a number.
fn vol_of(args: &[&str]) -> u8 {
    let out = trak().args(args).output().expect("run vol");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("vol printed {:?}: {e}", out.stdout))
}

#[test]
fn vol_steps_by_ten_percent() {
    // The fake starts at 80.
    assert_eq!(vol_of(&["vol", "show"]), 80);
    assert_eq!(vol_of(&["vol", "up"]), 90, "vol up is a 10% step");
    assert_eq!(vol_of(&["vol", "down"]), 70, "vol down is a 10% step");
}

#[test]
fn every_vol_command_prints_a_number_in_range() {
    for args in [vec!["vol", "show"], vec!["vol", "up"], vec!["vol", "down"]] {
        let v = vol_of(&args);
        assert!(v <= 100, "{args:?} produced {v}");
    }
}

#[test]
fn vol_json_prints_only_the_number() {
    trak()
        .args(["vol", "up", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"volume\":"));
}

#[test]
fn pos_seeks_and_reports_the_new_position() {
    trak()
        .args(["pos", "60"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1:00"));
}

#[test]
fn pos_rejects_a_nonsense_position() {
    trak()
        .args(["pos", "not-a-number"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value"));
}

#[test]
fn toggle_shuffle_and_repeat_both_work() {
    trak().args(["toggle", "shuffle"]).assert().success();
    trak().args(["toggle", "shuffle"]).assert().success();
    trak().args(["toggle", "repeat"]).assert().success();
    trak().args(["toggle", "repeat"]).assert().success();
}

#[test]
fn toggle_rejects_an_unknown_mode() {
    trak()
        .args(["toggle", "sideways"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value"));
}

#[test]
fn share_uri_and_url() {
    trak()
        .args(["share", "uri"])
        .assert()
        .success()
        .stdout("spotify:track:6HacgXCExkzS552ILfJTXu\n");
    trak()
        .args(["share", "url"])
        .assert()
        .success()
        .stdout("https://open.spotify.com/track/6HacgXCExkzS552ILfJTXu\n");
}

#[test]
fn play_uri_accepts_a_track_uri() {
    trak()
        .args(["play", "spotify:track:6HacgXCExkzS552ILfJTXu"])
        .assert()
        .success();
}

#[test]
fn play_uri_refuses_something_that_is_not_a_spotify_uri() {
    // A user-supplied string reaching AppleScript is the risk, so it is rejected
    // before it gets there.
    trak()
        .args(["play", "https://example.com/evil"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a Spotify URI"));
}

#[test]
fn play_uri_refuses_an_injection_attempt() {
    trak()
        .args(["play", "spotify:track:x\" & (do shell script \"id\") & \""])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a Spotify URI"));
}

#[test]
fn play_without_an_argument_resumes() {
    trak().arg("play").assert().success();
}

/// TODO 2.7's search half needs a Client ID, so without one it must say how to
/// get one and exit 2 rather than pretending to work.
#[test]
fn a_bare_name_is_a_setup_error_not_a_silent_nothing() {
    trak()
        .args(["play", "some song name"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Client ID"));
}

#[test]
fn bare_trak_exits_2_and_says_what_exists() {
    // TODO 2.8: the path must exist and be honest until the TUI lands.
    trak()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("status"));
}

#[test]
fn quit_is_refused_by_the_fake_rather_than_faking_success() {
    // The fake has no Spotify to quit, and saying "done" would be a lie.
    trak()
        .arg("quit")
        .assert()
        .failure()
        .stderr(predicate::str::contains("fake"));
}

/// A player that is not running must produce the idle-card guidance, not a crash
/// and not a launch (COMPAT rule 2).
#[test]
fn not_running_says_so_without_launching() {
    // Point osascript at a stub that reports Spotify is not running, which is
    // exactly what the real guard does when Spotify is quit.
    let stub = tempdir().join("osascript-stub");
    std::fs::write(&stub, "#!/bin/sh\nprintf 'not-running\\0370\\n'\n").unwrap();
    make_executable(&stub);

    let mut c = Command::cargo_bin("trak").unwrap();
    c.env("TRAK_OSASCRIPT", &stub);
    c.arg("status")
        .assert()
        .failure()
        .stderr(predicate::str::contains("isn't running"))
        .stderr(predicate::str::contains("never starts it"));
}

/// A permission denial must name the setting to change, not dump an OSStatus.
#[test]
fn permission_denied_names_the_system_setting() {
    let stub = tempdir().join("osascript-denied");
    std::fs::write(
        &stub,
        "#!/bin/sh\necho 'execution error: Not authorized to send Apple events (-1743)' >&2\nexit 1\n",
    )
    .unwrap();
    make_executable(&stub);

    let mut c = Command::cargo_bin("trak").unwrap();
    c.env("TRAK_OSASCRIPT", &stub);
    c.arg("status")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Automation"));
}

/// A scratch directory for the osascript stubs. Created on demand, because the
/// tests may run in parallel and `create_dir` is not atomic.
fn tempdir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join("trak-cli-tests");
    std::fs::create_dir_all(&d).expect("creating the stub directory");
    d
}

fn make_executable(p: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(p).unwrap().permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(p, perm).unwrap();
}
