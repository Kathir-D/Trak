//! `install.sh`, run end to end against a `file://` mirror of a release (TODO 9.9).
//!
//! The mirror holds a stand-in `trak` — a shell script that answers `--version` —
//! packed exactly the way `scripts/package-release.sh` packs the real one, so the
//! installer's download, checksum, unpack and copy are all exercised without the
//! network and without building a universal binary. `HOME` points into the
//! scratch directory, so nothing here can touch the real `~/.local`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const VERSION: &str = "9.9.9";

fn scratch(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!(
        "trak-install-tests-{}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed),
        name
    ));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).expect("make a scratch dir");
    d
}

fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("the command runs")
}

/// A release mirror: `trak-<v>-macos.tar.gz` and its `SHA256SUMS.txt`.
fn mirror(root: &Path) -> PathBuf {
    let stage = root.join("stage").join(format!("trak-{VERSION}"));
    fs::create_dir_all(&stage).unwrap();
    let bin = stage.join("trak");
    fs::write(&bin, format!("#!/bin/sh\necho 'trak {VERSION}'\n")).unwrap();
    assert!(
        run(Command::new("chmod").arg("0755").arg(&bin))
            .status
            .success()
    );
    let dist = root.join("dist");
    fs::create_dir_all(&dist).unwrap();
    let tarball = format!("trak-{VERSION}-macos.tar.gz");
    assert!(
        run(Command::new("tar")
            .arg("-czf")
            .arg(dist.join(&tarball))
            .arg("-C")
            .arg(root.join("stage"))
            .arg(format!("trak-{VERSION}")))
        .status
        .success()
    );
    let sums = run(Command::new("shasum")
        .arg("-a")
        .arg("256")
        .arg(&tarball)
        .current_dir(&dist));
    assert!(sums.status.success());
    fs::write(dist.join("SHA256SUMS.txt"), sums.stdout).unwrap();
    dist
}

/// `sh install.sh <args>` with a scratch HOME and the mirror as its release.
fn installer(root: &Path, dist: &Path, args: &[&str]) -> Command {
    let mut c = Command::new("sh");
    c.arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh"))
        .args(args)
        .env("HOME", root.join("home"))
        .env_remove("XDG_DATA_HOME")
        .env("TRAK_VERSION", VERSION)
        .env("TRAK_BASE_URL", format!("file://{}", dist.display()))
        .env("TRAK_INSTALL_DIR", root.join("bin"));
    c
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn installs_verifies_and_reports_the_version() {
    let root = scratch("happy");
    let dist = mirror(&root);
    let out = run(&mut installer(&root, &dist, &[]));
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("Checksum OK"), "{said}");
    assert!(
        said.contains(&format!("Installed trak {VERSION}")),
        "{said}"
    );
    let installed = root.join("bin/trak");
    let v = run(Command::new(&installed).arg("--version"));
    assert_eq!(
        String::from_utf8_lossy(&v.stdout).trim(),
        format!("trak {VERSION}")
    );
    // A scratch bin dir is not on PATH, so the hint must be there.
    assert!(said.contains("is not on your PATH"), "{said}");
    // The partial copy was renamed away, not left beside the binary.
    let names: Vec<_> = fs::read_dir(root.join("bin")).unwrap().flatten().collect();
    assert_eq!(names.len(), 1, "{names:?}");
}

#[test]
fn a_checksum_mismatch_installs_nothing() {
    let root = scratch("mismatch");
    let dist = mirror(&root);
    // A well-formed line for the right file, with a digest no tarball has.
    let forged = format!("{}  trak-{VERSION}-macos.tar.gz\n", "0".repeat(64));
    fs::write(dist.join("SHA256SUMS.txt"), forged).unwrap();
    let out = run(&mut installer(&root, &dist, &[]));
    let said = text(&out);
    assert!(!out.status.success(), "{said}");
    assert!(said.contains("checksum mismatch"), "{said}");
    assert!(!root.join("bin/trak").exists());
    assert!(!root.join("home/.local/share/trak").exists());
}

#[test]
fn a_sums_file_without_the_tarball_is_refused() {
    let root = scratch("nosum");
    let dist = mirror(&root);
    fs::write(dist.join("SHA256SUMS.txt"), "abc  something-else.tar.gz\n").unwrap();
    let out = run(&mut installer(&root, &dist, &[]));
    assert!(!out.status.success());
    assert!(text(&out).contains("no line for"), "{}", text(&out));
    assert!(!root.join("bin/trak").exists());
}

#[test]
fn a_missing_release_fails_cleanly() {
    let root = scratch("missing");
    let dist = root.join("empty");
    fs::create_dir_all(&dist).unwrap();
    let out = run(&mut installer(&root, &dist, &[]));
    assert!(!out.status.success());
    assert!(text(&out).contains("could not download"), "{}", text(&out));
}

#[test]
fn an_old_macos_is_refused_before_anything_downloads() {
    let root = scratch("oldmac");
    let dist = mirror(&root);
    let shims = root.join("shims");
    fs::create_dir_all(&shims).unwrap();
    for (product, ok) in [
        ("14.1", false),
        ("13.6.1", false),
        ("14.2", true),
        ("15.0", true),
    ] {
        let sw = shims.join("sw_vers");
        fs::write(&sw, format!("#!/bin/sh\necho {product}\n")).unwrap();
        assert!(
            run(Command::new("chmod").arg("0755").arg(&sw))
                .status
                .success()
        );
        let path = format!(
            "{}:{}",
            shims.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let out = run(installer(&root, &dist, &[]).env("PATH", path));
        assert_eq!(out.status.success(), ok, "{product}: {}", text(&out));
        if !ok {
            assert!(text(&out).contains("needs macOS 14.2"), "{}", text(&out));
            assert!(!text(&out).contains("Downloading"), "{}", text(&out));
        }
        let _ = fs::remove_file(root.join("bin/trak"));
    }
}

#[test]
fn uninstall_removes_exactly_what_was_installed() {
    let root = scratch("uninstall");
    let dist = mirror(&root);
    // A neighbour the installer did not write must survive.
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::write(root.join("bin/other-tool"), "keep me").unwrap();
    let config = root.join("home/.config/trak");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("config.toml"), "[display]\n").unwrap();

    assert!(run(&mut installer(&root, &dist, &[])).status.success());
    assert!(root.join("bin/trak").exists());

    let out = run(&mut installer(&root, &dist, &["--uninstall"]));
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(said.contains("Removed"), "{said}");
    assert!(!root.join("bin/trak").exists());
    assert!(root.join("bin/other-tool").exists());
    assert!(config.join("config.toml").exists());
    assert!(!root.join("home/.local/share/trak").exists());

    // A second run has nothing to do and says so rather than failing.
    let again = run(&mut installer(&root, &dist, &["--uninstall"]));
    assert!(again.status.success());
    assert!(
        text(&again).contains("Nothing to remove"),
        "{}",
        text(&again)
    );
}

#[test]
fn a_homebrew_link_is_never_overwritten() {
    let root = scratch("brewlink");
    let dist = mirror(&root);
    let cellar = root.join("Cellar/trak/0.1.0/bin");
    fs::create_dir_all(&cellar).unwrap();
    fs::write(cellar.join("trak"), "brew's").unwrap();
    fs::create_dir_all(root.join("bin")).unwrap();
    std::os::unix::fs::symlink(cellar.join("trak"), root.join("bin/trak")).unwrap();
    let out = run(&mut installer(&root, &dist, &[]));
    assert!(!out.status.success());
    assert!(text(&out).contains("Homebrew"), "{}", text(&out));
    assert_eq!(fs::read_to_string(cellar.join("trak")).unwrap(), "brew's");
}

#[test]
fn help_and_unknown_flags() {
    let root = scratch("help");
    let dist = root.join("unused");
    let out = run(&mut installer(&root, &dist, &["--help"]));
    assert!(out.status.success());
    let said = text(&out);
    assert!(
        said.contains("--uninstall") && said.contains("TRAK_VERSION"),
        "{said}"
    );
    let bad = run(&mut installer(&root, &dist, &["--frobnicate"]));
    assert_eq!(bad.status.code(), Some(2));
}
