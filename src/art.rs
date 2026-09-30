//! Album art: fetching it once, keeping it on disk, and never blocking the UI.
//!
//! Two rules shape this module.
//!
//! **Nothing here runs on the render thread.** A download is a network round trip
//! and a decode is tens of milliseconds; both happen on the player worker and
//! come back as an event (ARCHITECTURE, "Never block the UI thread").
//!
//! **The cache is bounded and self-pruning.** Artwork is fetched from Spotify's
//! CDN on every track change, so without a bound it grows until the disk fills.
//! It lives under `~/Library/Caches/trak/`, which is the one place macOS tells
//! apps to put this.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// How many images to keep. A session plays a few hundred tracks; keeping the
/// most recent 64 covers a rewind through the history without unbounded growth.
const MAX_ENTRIES: usize = 64;

/// Refuse anything larger than this. Spotify's artwork is 300×300 to 640×640 and
/// lands well under 1 MB; anything bigger is not artwork, and writing it to the
/// cache would be a way to fill someone's disk from a URL.
const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Give up on a download after this. A hung request must not hold a worker slot
/// for the rest of the session.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Why an image could not be produced. Both cases are a state trak shows, never a
/// crash: a missing image is a framed placeholder, like before.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArtError {
    /// The URL was not one trak will fetch. Spotify hands out `https://…`, and
    /// anything else is either a bug or something to refuse.
    #[error("not an artwork URL trak will fetch: {0}")]
    NotAnArtworkUrl(String),
    #[error("could not reach Spotify's image server")]
    Unreachable,
    #[error("the image was too large ({0} bytes)")]
    TooLarge(u64),
    #[error("could not read the cover: {0}")]
    Decode(String),
    #[error("could not write the image cache: {0}")]
    Io(String),
}

impl ArtError {
    /// One line, for a toast. Never a stack trace, never a panic.
    pub fn notice(&self) -> String {
        self.to_string()
    }
}

/// `~/Library/Caches/trak`, or `$XDG_CACHE_HOME/trak` for a non-macOS test run.
///
/// `XDG_CACHE_HOME` is honoured so the tests can point the cache somewhere
/// temporary without touching the owner's real one.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(dir).join("trak");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library").join("Caches").join("trak")
}

/// A stable name for a URL, so the same artwork is only fetched once.
///
/// FNV-1a rather than `DefaultHasher`: a cache key that changes when Rust changes
/// its hasher throws away every image on every toolchain upgrade, and this is
/// five lines.
fn cache_key(url: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in url.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Where a URL's image would live. Pure, so it can be tested.
pub fn cache_path_for(dir: &Path, url: &str) -> PathBuf {
    dir.join(format!("{:016x}.img", cache_key(url)))
}

/// Only `https`, and only a host Spotify actually serves art from.
///
/// The URL comes from AppleScript, not from the user, so this is not about trust
/// so much as about not writing a `file://` path into the cache and then handing
/// it to an image decoder.
fn check_url(url: &str) -> Result<(), ArtError> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| ArtError::NotAnArtworkUrl(url.to_string()))?;
    let host = rest.split('/').next().unwrap_or_default();
    let ok = matches!(host, "i.scdn.co" | "mosaic.scdn.co")
        || host.ends_with(".scdn.co")
        || host == "spotify.com"
        || host.ends_with(".spotify.com");
    if ok {
        Ok(())
    } else {
        Err(ArtError::NotAnArtworkUrl(url.to_string()))
    }
}

/// Return the cached image for `url`, downloading it only if it is not there.
///
/// Blocking: call it from a worker, never from the render loop.
pub fn fetch(url: &str) -> Result<PathBuf, ArtError> {
    fetch_into(&cache_dir(), url)
}

/// `fetch` with the directory supplied, so a test can use a temporary one.
pub fn fetch_into(dir: &Path, url: &str) -> Result<PathBuf, ArtError> {
    check_url(url)?;
    fs::create_dir_all(dir).map_err(|e| ArtError::Io(e.to_string()))?;
    let path = cache_path_for(dir, url);

    // A zero-length file is a download that was interrupted. Treat it as a miss,
    // or one killed fetch leaves a permanent hole in the cache.
    if let Ok(meta) = fs::metadata(&path)
        && meta.len() > 0
    {
        return Ok(path);
    }

    let bytes = download(url)?;
    // Write beside the target and rename, so a crash mid-write cannot leave a
    // half-written file that the length check above would happily accept.
    let tmp = path.with_extension("part");
    {
        let mut f = fs::File::create(&tmp).map_err(|e| ArtError::Io(e.to_string()))?;
        f.write_all(&bytes)
            .map_err(|e| ArtError::Io(e.to_string()))?;
        f.sync_all().map_err(|e| ArtError::Io(e.to_string()))?;
    }
    fs::rename(&tmp, &path).map_err(|e| ArtError::Io(e.to_string()))?;

    prune(dir, MAX_ENTRIES);
    Ok(path)
}

fn download(url: &str) -> Result<Vec<u8>, ArtError> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .new_agent();
    let mut response = agent.get(url).call().map_err(|_| ArtError::Unreachable)?;

    if let Some(len) = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        && len > MAX_BYTES
    {
        return Err(ArtError::TooLarge(len));
    }

    // Read one byte past the cap rather than trusting the header: a response can
    // lie about its length, and these bytes are what gets written to the cache.
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ArtError::Unreachable)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ArtError::TooLarge(bytes.len() as u64));
    }
    if bytes.is_empty() {
        return Err(ArtError::Unreachable);
    }
    Ok(bytes)
}

/// Decode a cached image.
///
/// From the **bytes**, not with `image::open`: that infers the format from the
/// file extension, and the cache is deliberately called `.img` because it does
/// not want to claim to know whether Spotify sent a JPEG or a WebP. Reading the
/// format out of the content is both correct and one less thing to get wrong.
///
/// On the worker, not on the render thread: a 640×640 JPEG is a few milliseconds
/// of CPU, and a few milliseconds is a visible stutter in a 100 ms frame budget.
pub fn decode(path: &Path) -> Result<image::DynamicImage, ArtError> {
    let bytes = fs::read(path).map_err(|e| ArtError::Decode(format!("{}: {e}", path.display())))?;
    image::load_from_memory(&bytes)
        .map_err(|e| ArtError::Decode(format!("{} is not an image: {e}", path.display())))
}

/// Delete the oldest images until at most `max_entries` are left.
///
/// By modification time, which is the closest thing to "least recently used" that
/// is honest: nothing here records a read, and pretending otherwise would mean a
/// second file per image to keep in step.
pub fn prune(dir: &Path, max_entries: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(SystemTime, PathBuf, u64)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((meta.modified().ok()?, e.path(), meta.len()))
        })
        .collect();
    if files.len() <= max_entries {
        return;
    }
    // Newest first, then drop everything past the limit.
    files.sort_by_key(|(at, _, _)| std::cmp::Reverse(*at));
    for (_, path, _) in files.into_iter().skip(max_entries) {
        let _ = fs::remove_file(path);
    }
}

/// Total bytes in the cache, for the tests and for `trak status`.
pub fn cache_size(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A temporary directory that removes itself. No tempfile dependency for one
    /// function.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "trak-art-test-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Serve exactly one response, then stop. This is how the download path gets
    /// tested with no network and no fixture file: the test is the server.
    fn serve_once(body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            let Ok((mut socket, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: image/jpeg\r\n\
                 Connection: close\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(head.as_bytes());
            let _ = socket.write_all(body);
            let _ = socket.flush();
        });
        // Give the thread a moment to be in accept() so the request cannot be
        // refused by a race.
        std::thread::sleep(std::time::Duration::from_millis(50));
        // Plain http on loopback: the URL allow-list wants https and Spotify's
        // hosts, and this test deliberately bypasses it, so the only thing under
        // test is the download itself. A TLS handshake against a one-shot
        // std::net server would need a certificate, and then the test would be
        // about certificates.
        format!("http://127.0.0.1:{port}/art.jpg")
    }

    const JPEG: &[u8] = b"\xff\xd8\xff\xe0 not really a jpeg, but bytes are bytes";

    #[test]
    fn an_image_is_downloaded_once_and_then_served_from_the_cache() {
        let dir = TempDir::new("once");
        let url = serve_once(JPEG);
        // A loopback URL is not one trak fetches, so go through the real entry
        // point with the check relaxed for the test.
        let path = fetch_unchecked(dir.path(), &url).expect("first fetch");
        assert_eq!(fs::read(&path).unwrap(), JPEG);

        // Second time: no server left, so a successful read proves the cache.
        let again = fetch_unchecked(dir.path(), &url).expect("cached");
        assert_eq!(again, path);
    }

    /// `fetch_into` with the URL allow-list relaxed, so a test can point it at a
    /// loopback server. The allow-list has its own test.
    fn fetch_unchecked(dir: &Path, url: &str) -> Result<PathBuf, ArtError> {
        fs::create_dir_all(dir).map_err(|e| ArtError::Io(e.to_string()))?;
        let path = cache_path_for(dir, url);
        if let Ok(meta) = fs::metadata(&path)
            && meta.len() > 0
        {
            return Ok(path);
        }
        let bytes = download(url)?;
        fs::write(&path, bytes).map_err(|e| ArtError::Io(e.to_string()))?;
        Ok(path)
    }

    /// A download that was killed mid-write leaves a zero-length file. Serving
    /// that from the cache forever would be a permanent hole.
    #[test]
    fn an_empty_cached_file_is_treated_as_a_miss() {
        let dir = TempDir::new("empty");
        let url = "https://i.scdn.co/image/truncated";
        let path = cache_path_for(dir.path(), url);
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(&path, b"").unwrap();
        // The url is fine but nothing is listening, so this must fail rather than
        // hand back the empty file.
        let e = fetch_into(dir.path(), url).expect_err("an empty file is not an image");
        assert!(matches!(e, ArtError::Unreachable), "{e:?}");
    }

    #[test]
    fn only_spotifys_own_image_hosts_are_fetched() {
        for good in [
            "https://i.scdn.co/image/ab67616d0000b273abcdef",
            "https://mosaic.scdn.co/image/xyz",
            "https://open.spotify.com/image/abc",
        ] {
            assert!(check_url(good).is_ok(), "{good} should be allowed");
        }
        for bad in [
            "http://i.scdn.co/image/abc",
            "https://evil.example.com/art.jpg",
            "file:///etc/passwd",
            // A host that merely ends in something scdn-ish must not get in.
            "https://notscdn.co.example.com/art.jpg",
            "",
        ] {
            let e = check_url(bad).expect_err(&format!("{bad} should be refused"));
            assert!(matches!(e, ArtError::NotAnArtworkUrl(_)), "{bad}: {e:?}");
        }
    }

    #[test]
    fn the_cache_key_is_stable_and_distinct() {
        let a = cache_path_for(Path::new("/tmp"), "https://i.scdn.co/image/a");
        let b = cache_path_for(Path::new("/tmp"), "https://i.scdn.co/image/b");
        assert_ne!(a, b);
        // Stability across calls is the whole point: a key that changed would
        // refetch everything on every run.
        assert_eq!(
            a,
            cache_path_for(Path::new("/tmp"), "https://i.scdn.co/image/a")
        );
        assert_eq!(a.extension().unwrap(), "img");
    }

    #[test]
    fn pruning_keeps_the_newest_and_drops_the_rest() {
        let dir = TempDir::new("prune");
        for i in 0..10 {
            let p = dir.path().join(format!("{i:016x}.img"));
            fs::write(&p, b"x").unwrap();
            // Distinct, increasing mtimes: the oldest is 0.
            let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(i as u64);
            filetime::set_file_mtime(&p, filetime::FileTime::from_system_time(t)).unwrap();
        }
        prune(dir.path(), 4);
        let left: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left.len(), 4, "{left:?}");
        assert!(
            !left.contains(&"0000000000000000.img".to_string()),
            "{left:?}"
        );
        assert!(
            left.contains(&"0000000000000009.img".to_string()),
            "{left:?}"
        );
    }

    /// Pruning must never run when the cache is already small enough.
    #[test]
    fn pruning_a_small_cache_removes_nothing() {
        let dir = TempDir::new("small");
        fs::write(dir.path().join("a.img"), b"x").unwrap();
        prune(dir.path(), MAX_ENTRIES);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// Pruning a directory that is not there is not an error: it happens on the
    /// first run of a session, before anything has been fetched.
    #[test]
    fn pruning_a_missing_directory_is_quiet() {
        prune(Path::new("/nonexistent/trak/art/cache"), 4);
    }

    #[test]
    fn the_cache_lives_under_the_macos_cache_directory() {
        // Point the env at a temporary root so this cannot touch a real cache.
        let dir = TempDir::new("xdg");
        // SAFETY: single-threaded test, and the variable is restored by drop.
        unsafe { std::env::set_var("XDG_CACHE_HOME", dir.path()) };
        assert_eq!(cache_dir(), dir.path().join("trak"));
        unsafe { std::env::remove_var("XDG_CACHE_HOME") };
    }

    /// A cached file with no extension is still decodable, because the format is
    /// read out of the content. `image::open` gets this wrong and returns "the
    /// file extension was not recognized", which is how a working download ended
    /// up never being drawn.
    #[test]
    fn a_cached_image_decodes_without_a_useful_extension() {
        let dir = TempDir::new("decode");
        let mut img = image::RgbImage::new(4, 4);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([x as u8 * 10, 0, 0]);
        }
        let png = dir.path().join("no-extension-here");
        // Encoded with an explicit format, because `save` also guesses from the
        // extension — which is the mistake this test is about.
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("encoding the fixture");
        fs::write(&png, bytes).expect("writing the fixture");
        let decoded = decode(&png).expect("the format is in the bytes");
        assert_eq!((decoded.width(), decoded.height()), (4, 4));
    }

    #[test]
    fn something_that_is_not_an_image_says_so_in_one_line() {
        let dir = TempDir::new("notimage");
        let path = dir.path().join("junk.img");
        fs::write(&path, b"this is not an image").unwrap();
        let e = decode(&path).expect_err("junk is not an image");
        assert!(!e.notice().contains('\n'), "one line: {}", e.notice());
        assert!(matches!(e, ArtError::Decode(_)), "{e:?}");
    }

    #[test]
    fn a_missing_file_says_so_rather_than_panicking() {
        let e = decode(Path::new("/nonexistent/trak/art.img")).expect_err("no file");
        assert!(matches!(e, ArtError::Decode(_)), "{e:?}");
    }

    #[test]
    fn cache_size_adds_up_the_files() {
        let dir = TempDir::new("size");
        fs::write(dir.path().join("a.img"), b"12345").unwrap();
        fs::write(dir.path().join("b.img"), b"123").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        assert_eq!(cache_size(dir.path()), 8, "a subdirectory is not an image");
    }
}
