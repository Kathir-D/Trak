//! Watching Spotify's `PlaybackStateChanged` distributed notification.
//!
//! This is what makes trak feel instant. A poll is 3–5 s and an AppleScript read
//! is ~430 ms, so without this a skip from Sonar or a media key would take
//! seconds to show. With it the update lands in about 170 ms
//! (docs/APPLESCRIPT.md §9).
//!
//! Three things the spike established, all of which shape this file:
//!
//! * **A plain un-bundled CLI receives it.** No app bundle, no `NSApplication`,
//!   no entitlement. `spikes/notify` is the proof.
//! * **`objc2-foundation` only generates the selector-based registration.** The
//!   block-taking variant is not in the crate, so an `NSObject` subclass is
//!   defined with `define_class!` and a selector is registered instead.
//! * **It is silent for seek, volume, shuffle and repeat.** Those four are what
//!   the poll exists for, which is why the poll stays (TODO 3.10).
//!
//! The observer is delivered on the main run loop, so the callback does nothing
//! but hand the data to a channel. Anything slower would stall the UI.

use std::sync::mpsc::{Receiver, Sender, channel};

use objc2::AnyThread;
use objc2::define_class;
use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{msg_send, sel};
use objc2_foundation::{
    NSDate, NSDistributedNotificationCenter, NSNotification, NSNotificationName, NSObject,
    NSRunLoop, NSString,
};

use crate::player::{PlaybackState, PlayerState};

/// The name Spotify posts. Confirmed on 1.3.1.234 (docs/APPLESCRIPT.md §9).
pub const NOTIFICATION_NAME: &str = "com.spotify.client.PlaybackStateChanged";

/// What a notification told us, normalised into the same shape as a poll.
///
/// Only the fields both sources share, so the app does not care which arrived.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackEvent {
    pub playback: PlaybackState,
    pub track_uri: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub position_secs: f64,
}

impl PlaybackEvent {
    /// Whether this event describes a different track than `state` is showing,
    /// which is what makes the update worth acting on.
    pub fn is_different_track(&self, state: &PlayerState) -> bool {
        state.track.uri.as_deref() != self.track_uri.as_deref()
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    pub struct Observer;

    unsafe impl NSObjectProtocol for Observer {}

    impl Observer {
        /// Called by AppKit on the main run loop. Does the minimum possible: read
        /// the userInfo, build a plain struct, hand it to the channel.
        #[unsafe(method(handleNotification:))]
        fn handle(&self, note: &NSNotification) {
            if let Some(tx) = sender_slot() {
                if let Some(event) = parse(note) {
                    let _ = tx.send(event);
                }
            }
        }
    }
);

/// The channel the callback writes to.
///
/// A `static` because the callback is an Objective-C method with no place to hang
/// state: `objc2`'s `define_class!` would need an ivar and a way to reach it from
/// a method. It is written once, before the run loop starts, and only read after.
static SENDER: std::sync::Mutex<Option<Sender<PlaybackEvent>>> = std::sync::Mutex::new(None);

fn sender_slot() -> Option<Sender<PlaybackEvent>> {
    // A poisoned lock only means a previous send panicked; the observer is still
    // worth keeping, so recover rather than propagate.
    SENDER.lock().ok()?.clone()
}

/// Read the notification's `userInfo` into a plain struct.
///
/// The real keys, from docs/APPLESCRIPT.md §9: `Player State` is **capitalised**
/// (`Playing`), `Track ID` is the **full URI**, `Duration` is **milliseconds** and
/// `Playback Position` is **seconds**.
fn parse(note: &NSNotification) -> Option<PlaybackEvent> {
    let ui = note.userInfo()?;

    // The values come back as untyped `AnyObject`, and their concrete classes
    // differ per key: the Swift spike showed `NSTaggedPointerString` for the text
    // keys and `__NSCFNumber` for the numeric ones. So each is downcast to the
    // class it is documented to be, and a wrong class yields None rather than a
    // wrong number.
    let text = |key: &str| -> Option<String> {
        let v = ui.objectForKey(&*NSString::from_str(key))?;
        v.downcast::<NSString>().ok().map(|s| s.to_string())
    };
    let num = |key: &str| -> Option<f64> {
        let v = ui.objectForKey(&*NSString::from_str(key))?;
        v.downcast::<objc2_foundation::NSNumber>()
            .ok()
            .map(|n| n.as_f64())
    };

    // `Player State` is `Playing` / `Paused`; `PlaybackState::parse` lowercases,
    // so both spellings work.
    let playback = text("Player State")
        .and_then(|s| PlaybackState::parse(&s))
        .unwrap_or(PlaybackState::Paused);

    let uri = text("Track ID").filter(|u| u.starts_with("spotify:track:"));

    Some(PlaybackEvent {
        playback,
        track_uri: uri,
        title: text("Name").unwrap_or_default(),
        artist: text("Artist").unwrap_or_default(),
        album: text("Album").unwrap_or_default(),
        // A missing duration must not become a divide-by-zero downstream.
        duration_ms: num("Duration").unwrap_or(0.0).max(0.0) as u64,
        position_secs: num("Playback Position").unwrap_or(0.0).max(0.0),
    })
}

/// A running subscription.
///
/// The observer is registered on the **calling (main) thread** and stays alive
/// for the process. That is not a shortcut: `NSDistributedNotificationCenter`
/// delivers on the main run loop regardless of which thread registered, so a
/// background run loop receives nothing at all. `spikes/notify` registered on the
/// main thread and worked, which is the version trak has to be.
///
/// The observer is deliberately never unregistered. trak's only subscriber is the
/// TUI, which exits by returning from its event loop and then exiting the
/// process; a `Retained` that outlives it costs nothing and avoids unregistering
/// from a thread other than the one that registered.
pub struct Subscription {
    rx: Receiver<PlaybackEvent>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

/// The registered observer, kept alive for the process.
static OBSERVER: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// Start watching, on the main thread.
///
/// Returns `None` if the notification cannot be registered, and the caller treats
/// that as "carry on with the poll alone" rather than as a failure. Notifications
/// are an optional extra; the app must work without them (ARCHITECTURE,
/// "Fail soft").
pub fn subscribe() -> Option<Subscription> {
    let (tx, rx) = channel();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thread_stop = std::sync::Arc::clone(&stop);

    let ok = OBSERVER.get_or_init(|| {
        unsafe {
            let Ok(mut slot) = SENDER.lock() else {
                return;
            };
            *slot = Some(tx);
            drop(slot);

            let observer: Retained<Observer> = msg_send![Observer::alloc(), init];
            let center = NSDistributedNotificationCenter::defaultCenter();
            let name = NSNotificationName::from_str(NOTIFICATION_NAME);
            center.addObserver_selector_name_object(
                &observer,
                sel!(handleNotification:),
                Some(&name),
                None,
            );
            // `observer` is retained by the notification center, and the OnceLock
            // keeps the process from ever unregistering.
            std::mem::forget(observer);
        }
    });
    let _ = ok;

    // The main thread has to keep pumping its run loop for anything to arrive, so
    // a thread is parked here purely to observe the stop flag; the run loop is
    // pumped by `pump_run_loop` from the event loop.
    let worker = std::thread::Builder::new()
        .name("trak-notify-watch".into())
        .spawn(move || {
            use std::sync::atomic::Ordering;
            while !thread_stop.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        })
        .ok()?;

    Some(Subscription {
        rx,
        stop,
        worker: Some(worker),
    })
}

/// Let the main run loop run for `secs`, so queued notifications are delivered.
///
/// This is what the TUI calls instead of a bare sleep while it waits for input:
/// it doubles as the frame pacing, so nothing is added to the loop's cost.
pub fn pump_run_loop(secs: f64) {
    let until = NSDate::dateWithTimeIntervalSinceNow(secs);
    NSRunLoop::currentRunLoop().runUntilDate(&until);
}

impl Subscription {
    /// Take an event if one has arrived. Never blocks.
    pub fn poll(&self) -> Option<PlaybackEvent> {
        self.rx.try_recv().ok()
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

/// Test hook: inject an event, so the TUI can be driven without a real Spotify.
#[cfg(test)]
impl Subscription {
    fn for_test() -> Option<Subscription> {
        let (tx, rx) = channel();
        if let Ok(mut slot) = SENDER.lock() {
            *slot = Some(tx);
        } else {
            return None;
        }
        Some(Subscription {
            rx,
            stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            worker: None,
        })
    }

    fn inject(&self, event: PlaybackEvent) {
        if let Some(tx) = sender_slot() {
            let _ = tx.send(event);
        }
    }
}

/// Fold a notification into a poll result, so the TUI has one code path.
///
/// The notification carries track metadata but **not** the artwork URL, so the
/// existing artwork is kept. That is the most important detail here: the
/// notification is not a replacement for a read, it is a faster *trigger* for one.
pub fn merge(state: &PlayerState, event: &PlaybackEvent) -> PlayerState {
    PlayerState {
        playback: event.playback,
        position_secs: event.position_secs,
        // Volume is never in the notification, and it is quantised, so the
        // previously known value is kept rather than guessed (COMPAT rule 5).
        volume: state.volume,
        shuffling_enabled: state.shuffling_enabled,
        repeating_enabled: state.repeating_enabled,
        track: crate::player::TrackInfo {
            title: event.title.clone(),
            artist: event.artist.clone(),
            album: event.album.clone(),
            album_artist: state.track.album_artist.clone(),
            duration_ms: event.duration_ms,
            track_number: state.track.track_number,
            disc_number: state.track.disc_number,
            // Popularity, play count and artwork are not in the notification, so
            // they stay as read. An advert has none of them anyway.
            popularity: state.track.popularity,
            play_count: state.track.play_count,
            artwork_url: state.track.artwork_url.clone(),
            uri: event.track_uri.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::parse::parse;
    use crate::testutil::fixture;

    fn event(playback: PlaybackState) -> PlaybackEvent {
        PlaybackEvent {
            playback,
            track_uri: Some("spotify:track:NEW".into()),
            title: "New Song".into(),
            artist: "New Artist".into(),
            album: "New Album".into(),
            duration_ms: 200_000,
            position_secs: 12.0,
        }
    }

    #[test]
    fn the_notification_name_is_the_one_spotify_posts() {
        assert_eq!(NOTIFICATION_NAME, "com.spotify.client.PlaybackStateChanged");
    }

    #[test]
    fn merging_keeps_what_the_notification_does_not_carry() {
        let state = parse(&fixture("playing_track.txt")).unwrap();
        let merged = merge(&state, &event(PlaybackState::Paused));

        // From the notification
        assert_eq!(merged.playback, PlaybackState::Paused);
        assert_eq!(merged.track.title, "New Song");
        assert_eq!(merged.track.uri.as_deref(), Some("spotify:track:NEW"));
        assert_eq!(merged.position_secs, 12.0);

        // Kept from the read, because the notification has no such key
        assert_eq!(
            merged.track.artwork_url, state.track.artwork_url,
            "the notification has no artwork url; losing it would blank the cover"
        );
        assert_eq!(merged.track.popularity, state.track.popularity);
        assert_eq!(merged.track.play_count, state.track.play_count);
        assert_eq!(merged.track.album_artist, state.track.album_artist);
        assert_eq!(merged.track.track_number, state.track.track_number);
        // and volume, which the notification never carries
        assert_eq!(merged.volume, state.volume);
        assert_eq!(merged.shuffling_enabled, state.shuffling_enabled);
    }

    #[test]
    fn a_different_track_is_detected() {
        let state = parse(&fixture("playing_track.txt")).unwrap();
        let e = event(PlaybackState::Playing);
        assert!(e.is_different_track(&state));

        let same = PlaybackEvent {
            track_uri: state.track.uri.clone(),
            ..event(PlaybackState::Playing)
        };
        assert!(!same.is_different_track(&state));
    }

    /// An advert posts the same notification but with no track URI. It must not
    /// be mistaken for a track, or the history would fill with unplayable rows.
    #[test]
    fn an_event_without_a_track_uri_is_not_a_track() {
        let e = PlaybackEvent {
            track_uri: None,
            ..event(PlaybackState::Playing)
        };
        let state = parse(&fixture("playing_ad.txt")).unwrap();
        assert!(!e.is_different_track(&state), "both have no URI");
        let merged = merge(&state, &e);
        assert!(merged.track.is_ad());
    }

    #[test]
    fn a_zero_duration_from_a_missing_key_does_not_divide_by_zero() {
        let state = parse(&fixture("playing_track.txt")).unwrap();
        let e = PlaybackEvent {
            duration_ms: 0,
            ..event(PlaybackState::Playing)
        };
        let merged = merge(&state, &e);
        assert_eq!(merged.progress(), 0.0);
    }

    #[test]
    fn subscribing_and_polling_works_without_spotify() {
        // The observer needs a run loop, but a test only needs the channel: this
        // proves the wiring and the fail-soft contract without a main loop.
        let Some(sub) = Subscription::for_test() else {
            panic!("the sender slot should be available");
        };
        sub.inject(event(PlaybackState::Playing));
        let got = sub.poll().expect("the injected event");
        assert_eq!(got.title, "New Song");
        // and it drains to empty rather than repeating
        assert!(sub.poll().is_none());
    }

    #[test]
    fn polling_an_idle_subscription_is_simply_empty() {
        let sub = Subscription::for_test().expect("a subscription");
        assert!(sub.poll().is_none());
    }

    /// Dropping must not hang or leak, whether or not the thread ever started.
    #[test]
    fn dropping_a_subscription_is_clean() {
        let sub = Subscription::for_test().expect("a subscription");
        drop(sub);
    }
}
