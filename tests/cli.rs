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

/// A `trak` invocation with the fake player *and* the fake library: the state
/// `trak play <name>` needs, a machine with something to search. `--fake` on
/// its own deliberately still has no library — that is the state of a machine
/// with no Client ID, and the setup path has to stay testable too.
fn trak_with_library() -> Command {
    let mut c = trak();
    c.arg("--fake-library");
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

    // SPEC §9's qualified play spellings, or a user who read the spec cannot
    // find them.
    let play = trak()
        .arg("play")
        .arg("--help")
        .output()
        .expect("run play --help");
    let p = String::from_utf8_lossy(&play.stdout);
    for sub in ["album", "artist", "list", "uri"] {
        assert!(p.contains(sub), "play --help is missing `{sub}`:\n{p}");
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

/// A name needs search, and `--fake` alone still means no library — the state
/// of every machine with no Client ID — so it must say how to get one and exit
/// 2 rather than pretending to have played something. The other half of the
/// behaviour, a name with a library to search, is pinned by the tests below.
///
/// `XDG_CONFIG_HOME` is pinned to an empty directory: the real library half of
/// `play` reads the token file wherever the user's variables say it is, and a
/// test must not depend on whether this machine has logged in.
#[test]
fn a_bare_name_is_a_setup_error_not_a_silent_nothing() {
    let xdg = tempdir().join("xdg-empty");
    std::fs::create_dir_all(&xdg).unwrap();
    trak()
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["play", "some song name"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Client ID"));
}

/// The setup message quotes the command back, and each group has to quote its
/// own — telling someone who asked for an album that `trak play "x"` needs a
/// Client ID reads as though the group had been dropped on the floor.
#[test]
fn the_setup_message_quotes_the_spelling_that_was_typed() {
    let xdg = tempdir().join("xdg-empty-groups");
    std::fs::create_dir_all(&xdg).unwrap();
    for (args, quoted) in [
        (vec!["play", "x"], "trak play \"x\""),
        (vec!["play", "album", "x"], "trak play album \"x\""),
        (vec!["play", "artist", "x"], "trak play artist \"x\""),
        (vec!["play", "list", "x"], "trak play list \"x\""),
    ] {
        trak()
            .env("XDG_CONFIG_HOME", &xdg)
            .args(&args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains(quoted));
    }
}

/// A token file nobody could use is the pre-setup state too — the store refuses
/// a loose mode without reading it and a file it cannot parse without using it
/// — and both must land in the same setup message, not a crash and not a
/// search. This is the "fail soft" rule for the one command that reads the
/// token file.
#[test]
fn an_unusable_token_file_is_still_the_setup_state() {
    use std::os::unix::fs::PermissionsExt as _;
    let xdg = tempdir().join("xdg-unusable");
    std::fs::create_dir_all(xdg.join("trak")).unwrap();
    let token = xdg.join("trak").join("token.json");

    // World-readable: refused on mode alone, bytes never read.
    std::fs::write(&token, "irrelevant").unwrap();
    let mut loose = std::fs::metadata(&token).unwrap().permissions();
    loose.set_mode(0o644);
    std::fs::set_permissions(&token, loose).unwrap();
    trak()
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["play", "some song name"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Client ID"));

    // Owner-only but not JSON: read, refused, left on disk.
    std::fs::write(&token, "{\"not\":\"a token\"}").unwrap();
    let mut tight = std::fs::metadata(&token).unwrap().permissions();
    tight.set_mode(0o600);
    std::fs::set_permissions(&token, tight).unwrap();
    trak()
        .env("XDG_CONFIG_HOME", &xdg)
        .args(["play", "some song name"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Client ID"));
}

/// TODO 7.12: a bare name searches tracks, plays the best match and says what
/// it chose — the one play spelling that is not silent, because "the best
/// match" is a decision the user has to be able to see.
#[test]
fn play_a_name_searches_and_prints_what_it_chose() {
    trak_with_library()
        .args(["play", "teardrop"])
        .assert()
        .success()
        .stdout("Playing Teardrop — Massive Attack (Mezzanine)\n");
}

/// The query names both a song and the album it sits on; the track search must
/// prefer the song actually named that, which sits behind two results that only
/// match through their album.
#[test]
fn play_a_name_prefers_the_row_named_after_the_query() {
    trak_with_library()
        .args(["play", "mezzanine"])
        .assert()
        .success()
        .stdout("Playing Mezzanine — Massive Attack (Mezzanine)\n");
}

/// shpotify joined its arguments into one phrase, so an unquoted multi-word
/// search is one query, not a usage error.
#[test]
fn play_joins_several_words_into_one_search() {
    trak_with_library()
        .args(["play", "massive", "attack"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("Playing Teardrop"));
}

/// `album|artist|list` search their own group (SPEC §9) and print the row's
/// one-line form, which is the same line the tabs draw.
#[test]
fn play_album_artist_and_list_search_their_own_group() {
    trak_with_library()
        .args(["play", "album", "mezzanine"])
        .assert()
        .success()
        .stdout("Playing Mezzanine — Massive Attack (1998)\n");
    trak_with_library()
        .args(["play", "artist", "massive attack"])
        .assert()
        .success()
        .stdout("Playing Massive Attack\n");
    trak_with_library()
        .args(["play", "list", "mass"])
        .assert()
        .success()
        .stdout("Playing Massive Attack on Repeat · 3 tracks\n");
}

/// A miss is a clean failure with shpotify's own sentence, not a play of
/// whatever happened to be nearby.
#[test]
fn play_reports_a_miss_as_a_failure() {
    trak_with_library()
        .args(["play", "nothing by this name"])
        .assert()
        .code(1)
        .stderr("No results when searching for \"nothing by this name\"\n");
    // And the qualified spellings miss the same way, not with a different
    // message per group.
    trak_with_library()
        .args(["play", "artist", "nobody of this name"])
        .assert()
        .code(1)
        .stderr("No results when searching for \"nobody of this name\"\n");
}

/// A URI never becomes a search term just because a library exists: both the
/// bare spelling and shpotify's `uri` one play straight through, silently, the
/// way the README documents.
#[test]
fn a_uri_still_bypasses_search() {
    trak_with_library()
        .args(["play", "spotify:track:6HacgXCExkzS552ILfJTXu"])
        .assert()
        .success()
        .stdout("");
    trak_with_library()
        .args(["play", "uri", "spotify:album:5nMdc39z78kifAc5WXv9Yj"])
        .assert()
        .success()
        .stdout("");
}

/// A group with no name is a usage error, exit 2, the code every clap usage
/// error already exits with.
#[test]
fn a_group_with_no_name_is_a_usage_error() {
    trak().args(["play", "album"]).assert().code(2);
    trak().args(["play", "list"]).assert().code(2);
}

/// The whole chain against the real player path: the search finds the album,
/// and the URI AppleScript receives is the album's context URI, not a track's.
/// The stub records the one script it is fed, which is the only place the URI
/// is visible from outside the process.
#[test]
fn play_album_puts_the_album_uri_in_the_applescript() {
    let stub = tempdir().join("osascript-recorded");
    let recorded = tempdir().join("recorded-play-album");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\ncat > '{}'\nprintf 'ok\\n'\n",
            recorded.display()
        ),
    )
    .unwrap();
    make_executable(&stub);

    let mut c = Command::cargo_bin("trak").unwrap();
    c.env("TRAK_OSASCRIPT", &stub);
    // The real player with the fake library: search without a network, play
    // through the same script path production uses.
    c.arg("--fake-library")
        .args(["play", "album", "mezzanine"])
        .assert()
        .success()
        .stdout("Playing Mezzanine — Massive Attack (1998)\n");

    let script = std::fs::read_to_string(&recorded).unwrap();
    assert!(
        script.contains(r#"play track "spotify:album:5nMdc39z78kifAc5WXv9Yj""#),
        "the album URI never reached AppleScript:\n{script}"
    );
}

/// Bare `trak` is the TUI, and a TUI cannot run on a pipe. It must say so and
/// point at the commands that do work, rather than entering raw mode and looking
/// hung.
#[test]
fn bare_trak_without_a_terminal_says_so_and_suggests_the_cli() {
    trak()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("needs a terminal"))
        .stderr(predicate::str::contains("trak status"));
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
