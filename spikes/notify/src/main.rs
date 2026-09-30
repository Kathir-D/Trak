// TODO 1.3: can a *plain Rust CLI process* — no app bundle, no Xcode project,
// just `cargo run` — observe Spotify's distributed notification?
//
// The doubt TODO 1.3 raises is that distributed notifications only reach bundled
// apps. Answer: a bare binary registers fine. This is the shape trak's
// `player/notify.rs` will take, reduced to what the spike needed to prove.
//
// Uses the selector-based registration (addObserver:selector:name:object:)
// because objc2-foundation 0.3 does not generate the block-taking variant of
// NSDistributedNotificationCenter's addObserver. That is fine: trak already owns
// a main run loop for the TUI.

use objc2::AnyThread;
use objc2::define_class;
use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{msg_send, sel};
use objc2_foundation::{
    NSDate, NSDistributedNotificationCenter, NSNotification, NSNotificationName, NSObject,
    NSRunLoop, NSString,
};
use std::io::Write;

define_class!(
    #[unsafe(super(NSObject))]
    pub struct Observer;

    unsafe impl NSObjectProtocol for Observer {}

    impl Observer {
        #[unsafe(method(handleNotification:))]
        fn handle(&self, note: &NSNotification) {
            let mut out = String::new();
            if let Some(ui) = note.userInfo() {
                for k in ui.allKeys().iter() {
                    if let Ok(ks) = k.downcast::<NSString>() {
                        let v = ui.objectForKey(&*ks);
                        out.push_str(&format!("      {ks} = {v:?}\n"));
                    }
                }
            }
            let state = note
                .userInfo()
                .and_then(|u| u.objectForKey(&*NSString::from_str("Player State")))
                .map(|v| format!("{v:?}"))
                .unwrap_or_else(|| "?".into());
            println!("EVENT {state}\n{out}");
            let _ = std::io::stdout().flush();
        }
    }
);

fn main() {
    unsafe {
        let observer: Retained<Observer> = msg_send![Observer::alloc(), init];

        let center = NSDistributedNotificationCenter::defaultCenter();
        let name = NSNotificationName::from_str("com.spotify.client.PlaybackStateChanged");
        center.addObserver_selector_name_object(
            &observer,
            sel!(handleNotification:),
            Some(&name),
            None,
        );

        println!("listening 16s for com.spotify.client.PlaybackStateChanged");
        let _ = std::io::stdout().flush();

        let until = NSDate::dateWithTimeIntervalSinceNow(16.0);
        NSRunLoop::currentRunLoop().runUntilDate(&until);
        println!("done");
    }
}
