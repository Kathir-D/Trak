// TODO 1.5 follow-up: bypass `cidre` for the four `CATapDescription` initialisers
// and find out whether `!obj` comes from cidre's binding or from macOS.
//
// Why this binary exists. `docs/AUDIO-TAP.md` §3b recorded that every
// process-specific tap description fails with 560947818 = `!obj`, while an *empty*
// pid list is accepted, at every NSNumber width. Two different things could cause
// that, and they call for opposite responses:
//
//   * cidre's `TapDesc` resolves its class with `objc_getClass("CA_TAP_DESCRIPTION")`
//     -- a name CoreAudio does not publish -- so the whole tap description may be a
//     locally-registered class that never talks to the real `CATapDescription`.
//   * The argument may be the wrong *kind* of number.
//
// So this file builds the description itself, with objc2, straight off the real
// `CATapDescription` class, and hands the result to the same
// `AudioHardwareCreateProcessTap` cidre calls. Nothing about the tap description
// comes from cidre any more; everything after it (tap uid, ASBD, aggregate device,
// IO proc) still does, so a success here is a real capture and not a stub.
//
// The one thing this file found: the header says the argument is not a pid.
//
//     @param processesObjectIDsToIncludeInTap
//         An NSArray of NSNumbers where each NSNumber holds an AudioObjectID of the
//         process object to include in the tap
//
// A pid has to be translated into that AudioObjectID first, via
// `kAudioHardwarePropertyTranslatePIDToProcessObject` ('id2p'). Passing the pid
// directly is what every earlier attempt did, and it is why `!obj` appeared for a
// non-empty list and not for an empty one. `TRAK_LIST` picks which of the two is
// handed to the selector so the difference is observable rather than argued about.

use std::ffi::c_void;
use std::time::Duration;

use cidre::{cf, core_audio as ca, ns};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{msg_send, sel};
use objc2_foundation::{NSArray, NSNumber, NSString};

// ---------------------------------------------------------------------------
// The bits of CoreAudio the ObjC layer sits on top of. cidre has bindings for
// all of this, but calling them directly keeps the point of the spike honest:
// the description is built by objc2 and nothing else.

type AudioObjectID = u32;

const K_AUDIO_OBJECT_SYSTEM_OBJECT: AudioObjectID = 1;
const SCOPE_GLOBAL: u32 = u32::from_be_bytes(*b"glob");
const PRCS_LIST: u32 = u32::from_be_bytes(*b"prs#");
const TRANSLATE_PID_TO_PROCESS_OBJECT: u32 = u32::from_be_bytes(*b"id2p");
const PROCESS_PROPERTY_PID: u32 = u32::from_be_bytes(*b"ppid");

#[repr(C)]
#[derive(Clone, Copy)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

unsafe extern "C" {
    fn AudioObjectGetPropertyData(
        object: AudioObjectID,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn AudioObjectHasProperty(object: AudioObjectID, address: *const PropertyAddress) -> bool;
}

/// The four-character code of an OSStatus, so `!obj` prints as `!obj`.
fn fourcc(status: i32) -> String {
    let b = (status as u32).to_be_bytes();
    if b.iter().all(|c| (0x20..0x7f).contains(c)) {
        format!(
            "'{}' 0x{:08X}",
            String::from_utf8_lossy(&b),
            status as u32
        )
    } else {
        format!("0x{:08X}", status as u32)
    }
}

fn property_u32(object: AudioObjectID, address: PropertyAddress) -> Result<u32, i32> {
    let mut size = std::mem::size_of::<u32>() as u32;
    let mut out: u32 = 0;
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            &mut out as *mut u32 as *mut c_void,
        )
    };
    if status == 0 { Ok(out) } else { Err(status) }
}

/// Every AudioObjectID that currently names a Core Audio *process object*.
///
/// This is the list the header means when it says "AudioObjectID of the process
/// object". Note how different it looks from the pids they correspond to.
fn process_objects() -> Result<Vec<AudioObjectID>, i32> {
    let address = PropertyAddress {
        selector: PRCS_LIST,
        scope: SCOPE_GLOBAL,
        element: 0,
    };
    let mut size: u32 = 0;
    let status = unsafe {
        AudioObjectGetPropertyData(
            K_AUDIO_OBJECT_SYSTEM_OBJECT,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            std::ptr::null_mut(),
        )
    };
    if status != 0 {
        return Err(status);
    }
    let mut buf = vec![0u8; size as usize];
    let status = unsafe {
        AudioObjectGetPropertyData(
            K_AUDIO_OBJECT_SYSTEM_OBJECT,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            buf.as_mut_ptr() as *mut c_void,
        )
    };
    if status != 0 {
        return Err(status);
    }
    let count = (size as usize) / std::mem::size_of::<AudioObjectID>();
    let mut out = Vec::with_capacity(count);
    for chunk in buf.chunks_exact(std::mem::size_of::<AudioObjectID>()) {
        out.push(u32::from_ne_bytes(chunk.try_into().unwrap()));
    }
    Ok(out)
}

fn process_pid(object: AudioObjectID) -> Option<i32> {
    property_u32(
        object,
        PropertyAddress {
            selector: PROCESS_PROPERTY_PID,
            scope: SCOPE_GLOBAL,
            element: 0,
        },
    )
    .ok()
    .map(|p| p as i32)
}

/// `kAudioHardwarePropertyTranslatePIDToProcessObject`: the one call the earlier
/// attempts were missing. Returns `kAudioObjectUnknown` (0) for a pid that names
/// no Core Audio process -- notably, a process that is not an audio client.
fn process_object_for_pid(pid: i32) -> Result<AudioObjectID, i32> {
    let qualifier = pid as u32;
    let address = PropertyAddress {
        selector: TRANSLATE_PID_TO_PROCESS_OBJECT,
        scope: SCOPE_GLOBAL,
        element: 0,
    };
    let mut size = std::mem::size_of::<u32>() as u32;
    let mut out: AudioObjectID = 0;
    let status = unsafe {
        AudioObjectGetPropertyData(
            K_AUDIO_OBJECT_SYSTEM_OBJECT,
            &address,
            std::mem::size_of::<u32>() as u32,
            &qualifier as *const u32 as *const c_void,
            &mut size,
            &mut out as *mut AudioObjectID as *mut c_void,
        )
    };
    if status != 0 {
        return Err(status);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The bypass: CATapDescription through objc2, no cidre.

/// `cidre::core_audio::TapDesc` is a newtype over `objc::Id`, which is itself a
/// `#[repr(transparent)]` pointer wrapper. A single-field Rust struct has the same
/// layout as its field, so a `&TapDesc` is just a typed view of the object pointer.
/// That is the whole reason an objc2-built description can be handed to cidre's
/// `create_process_tap`, which is what keeps the rest of this file identical to
/// `main.rs`. Checked at run time rather than asserted, so a cidre upgrade that
/// changes the layout fails loudly instead of corrupting memory.
fn as_cidre_tap_desc(obj: &AnyObject) -> &ca::TapDesc {
    assert_eq!(
        std::mem::size_of::<ca::TapDesc>(),
        std::mem::size_of::<*mut c_void>(),
        "cidre changed the layout of TapDesc; the objc2 -> cidre handoff is no longer a pointer cast"
    );
    // SAFETY: `TapDesc` adds no fields and has no Drop, so a reference to it is
    // exactly a reference to the same bytes objc2 handed us.
    unsafe { &*(obj as *const AnyObject as *const ca::TapDesc) }
}

/// Which of the four initialisers to call. These are the shapes the tap
/// description can take; nothing else in the spike picks between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// `initMonoMixdownOfProcesses:` -- include-list, mono. The shape trak wants.
    MonoMixdown,
    /// `initStereoMixdownOfProcesses:` -- include-list, stereo.
    StereoMixdown,
    /// `initMonoGlobalTapButExcludeProcesses:` -- exclude-list, mono.
    MonoGlobalExclude,
    /// `initStereoGlobalTapButExcludeProcesses:` -- exclude-list, stereo.
    StereoGlobalExclude,
}

impl Shape {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "mono-mixdown" => Self::MonoMixdown,
            "stereo-mixdown" => Self::StereoMixdown,
            "global-mono" | "mono-global-exclude" => Self::MonoGlobalExclude,
            "global-stereo" | "stereo-global-exclude" => Self::StereoGlobalExclude,
            _ => return None,
        })
    }

    fn selector(self) -> &'static str {
        match self {
            Self::MonoMixdown => "initMonoMixdownOfProcesses:",
            Self::StereoMixdown => "initStereoMixdownOfProcesses:",
            Self::MonoGlobalExclude => "initMonoGlobalTapButExcludeProcesses:",
            Self::StereoGlobalExclude => "initStereoGlobalTapButExcludeProcesses:",
        }
    }

    fn is_include_list(self) -> bool {
        matches!(self, Self::MonoMixdown | Self::StereoMixdown)
    }

    fn describe(self) -> &'static str {
        match self {
            Self::MonoMixdown => "include-list, mono  (the shape SPEC 7 wants)",
            Self::StereoMixdown => "include-list, stereo",
            Self::MonoGlobalExclude => "exclude-list, mono",
            Self::StereoGlobalExclude => "exclude-list, stereo",
        }
    }
}

/// What the numbers handed to the selector mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListKind {
    /// Nothing at all. The control: this is the one case known to be accepted.
    Empty,
    /// The pids themselves. What every earlier attempt passed, at every NSNumber
    /// width.
    Pids,
    /// The `AudioObjectID` of the process object for each pid, which is what
    /// `CATapDescription.h` documents.
    Objects,
}

impl ListKind {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "empty" => Self::Empty,
            "pids" => Self::Pids,
            "objects" => Self::Objects,
            _ => return None,
        })
    }
}

/// Build a `CATapDescription` through objc2 and return the raw object.
///
/// Deliberately *not* `#[objc2::define_class!]`: the class already exists in
/// CoreAudio and `define_class!` is for making new ones. Getting the class handle
/// out of the runtime and sending the real selector is all the bypass needs, and it
/// is short enough to read.
fn make_tap_desc(shape: Shape, numbers: &[i64]) -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"CATapDescription")?;

    let arr: Retained<NSArray<NSNumber>> = NSArray::from_retained_slice(
        &numbers
            .iter()
            .map(|n| NSNumber::new_i64(*n))
            .collect::<Vec<_>>(),
    );
    println!(
        "  array argument: {} NSNumber(s) [{}]",
        numbers.len(),
        numbers
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // `alloc` and the `init` family return +1 objects, so the raw pointer is
    // retained into an owning handle exactly once -- no autorelease round trip.
    // SAFETY: the class is CATapDescription (checked by the caller) and the
    // selector and argument type match CATapDescription.h.
    let allocated: *mut AnyObject = unsafe { msg_send![cls, alloc] };
    let allocated: Retained<AnyObject> =
        unsafe { Retained::retain(allocated) }.expect("alloc returned nil");

    let raw: *mut AnyObject = unsafe {
        match shape {
            Shape::MonoMixdown => msg_send![&*allocated, initMonoMixdownOfProcesses: &*arr],
            Shape::StereoMixdown => msg_send![&*allocated, initStereoMixdownOfProcesses: &*arr],
            Shape::MonoGlobalExclude => {
                msg_send![&*allocated, initMonoGlobalTapButExcludeProcesses: &*arr]
            }
            Shape::StereoGlobalExclude => {
                msg_send![&*allocated, initStereoGlobalTapButExcludeProcesses: &*arr]
            }
        }
    };
    if raw.is_null() {
        println!("  initialiser returned nil (selector {} not implemented)", shape.selector());
        return None;
    }
    // `init` consumes the receiver, so this is the same object under a +1 count
    // the caller now owns; retaining once is the right accounting.
    unsafe { Retained::retain(raw) }
}

/// Read the description back through objc2 and print it, so the object is shown to
/// exist and to carry what was put in it rather than merely being non-nil.
fn dump_tap_desc(desc: &AnyObject) {
    // SAFETY: read-only accessors on CATapDescription. `processes` is an
    // `NSArray<NSNumber*>` copy, and `as_i64` on it is valid because the property
    // is documented to hold numbers.
    unsafe {
        let processes: Retained<NSArray<NSNumber>> = msg_send![desc, processes];
        let count = processes.count();
        let mut got = Vec::with_capacity(count as usize);
        for i in 0..count {
            let n: Retained<NSNumber> = processes.objectAtIndex(i);
            got.push(n.as_i64());
        }
        println!("  description.processes = [{got:?}]");
        let mono: bool = msg_send![desc, isMono];
        let exclusive: bool = msg_send![desc, isExclusive];
        let mixdown: bool = msg_send![desc, isMixdown];
        let private: bool = msg_send![desc, isPrivate];
        let name: Option<Retained<NSString>> = msg_send![desc, name];
        println!(
            "  isMono={mono} isExclusive={exclusive} isMixdown={mixdown} isPrivate={private}"
        );
        println!(
            "  name={}",
            name.map(|n| n.to_string()).unwrap_or_else(|| "<nil>".into())
        );
    }
}

// ---------------------------------------------------------------------------
// The capture half, unchanged from main.rs: cidre does the IO because that is not
// what this experiment is about, and reusing it keeps the two spikes comparable.

struct Ctx {
    frames: std::sync::atomic::AtomicU64,
    running: std::sync::atomic::AtomicBool,
    last_report: std::sync::atomic::AtomicU64,
    peak: f32,
    sample_rate: f64,
}

fn measure(list: &cidre::cat::AudioBufList<2>) -> (f64, f32, usize) {
    let mut sum = 0.0f64;
    let mut peak = 0.0f32;
    let mut n = 0usize;
    for i in 0..list.number_buffers as usize {
        let b = list.buffers[i];
        if b.data.is_null() || b.data_bytes_size == 0 {
            continue;
        }
        // SAFETY: Core Audio guarantees `data` covers `data_bytes_size` valid bytes
        // for the duration of the callback, and this only reads.
        let samples = unsafe {
            std::slice::from_raw_parts(b.data as *const f32, b.data_bytes_size as usize / 4)
        };
        for s in samples {
            sum += (s * s) as f64;
            let a = s.abs();
            if a > peak {
                peak = a;
            }
        }
        n += samples.len();
    }
    let rms = if n == 0 { 0.0 } else { (sum / n as f64).sqrt() };
    (rms, peak, n)
}

extern "C" fn on_audio(
    _device: ca::Device,
    _now: &cidre::cat::AudioTimeStamp,
    input_data: &cidre::cat::AudioBufList<2>,
    _input_time: &cidre::cat::AudioTimeStamp,
    _output_data: &mut cidre::cat::AudioBufList<2>,
    _output_time: &cidre::cat::AudioTimeStamp,
    ctx: Option<&mut Ctx>,
) -> cidre::os::Status {
    use std::sync::atomic::Ordering;

    let Some(ctx) = ctx else {
        return Default::default();
    };
    if !ctx.running.load(Ordering::Relaxed) {
        return Default::default();
    }
    let (rms, peak, n) = measure(input_data);
    if n == 0 {
        return Default::default();
    }
    if peak > ctx.peak {
        ctx.peak = peak;
    }
    let total = ctx.frames.fetch_add(n as u64, Ordering::Relaxed);
    let window = (ctx.sample_rate * 0.25) as u64;
    if window > 0 && total / window != ctx.last_report.load(Ordering::Relaxed) / window {
        ctx.last_report.store(total, Ordering::Relaxed);
        let db = 20.0 * rms.max(1e-9).log10();
        let bars = ((rms * 12.0).clamp(0.0, 1.0) * 44.0) as usize;
        println!(
            "  rms={rms:.6}  {db:>7.2} dBFS  peak={:.4}  |{:<44}|",
            ctx.peak,
            "#".repeat(bars)
        );
        use std::io::Write;
        let _ = std::io::stdout().flush();
        ctx.peak = 0.0;
    }
    Default::default()
}

/// Build the aggregate device over the tap and run the IO proc for a fixed window.
/// Returns the number of float samples that arrived.
fn capture(desc: &AnyObject, seconds: u64) -> Result<u64, String> {
    let output_device = ca::System::default_output_device().map_err(|e| e.to_string())?;
    let output_uid = output_device.uid().map_err(|e| e.to_string())?;

    // The description was built by objc2; from here cidre takes over, which is the
    // point -- only the description is bypassed.
    let tap = as_cidre_tap_desc(desc)
        .create_process_tap()
        .map_err(|e| format!("create_process_tap: {} {}", e.0.get(), fourcc(e.0.get() as i32)))?;
    println!(
        "  tap created: {}",
        tap.name().map(|n| n.to_string()).unwrap_or_default()
    );

    let tap_uid = tap.uid().map_err(|e| format!("tap uid: {}", e.0))?;
    let asbd = tap.asbd().map_err(|e| format!("tap asbd: {}", e.0))?;
    let sample_rate = asbd.sample_rate;
    println!(
        "  format: {sample_rate} Hz, {} ch, {} bit, flags={:?}",
        asbd.channels_per_frame, asbd.bits_per_channel, asbd.format_flags
    );

    use cidre::core_audio::{aggregate_device_keys as agg, sub_device_keys as sub};
    let sub_device =
        cf::DictionaryOf::with_keys_values(&[sub::uid()], &[output_uid.as_type_ref()]);
    let sub_tap = cf::DictionaryOf::with_keys_values(&[sub::uid()], &[tap_uid.as_type_ref()]);
    let dict = cf::DictionaryOf::with_keys_values(
        &[
            agg::is_private(),
            agg::is_stacked(),
            agg::tap_auto_start(),
            agg::name(),
            agg::main_sub_device(),
            agg::uid(),
            agg::sub_device_list(),
            agg::tap_list(),
        ],
        &[
            cf::Boolean::value_true().as_type_ref(),
            cf::Boolean::value_false(),
            cf::Boolean::value_true(),
            cf::str!(c"trak tap (objc2 bypass)").as_type_ref(),
            &output_uid,
            &cf::Uuid::new().to_cf_string(),
            &cf::ArrayOf::from_slice(&[sub_device.as_ref()]),
            &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
        ],
    );
    let agg_device = ca::AggregateDevice::with_desc(&dict)
        .map_err(|e| format!("AggregateDevice::with_desc: {}", e.0))?;
    println!("  aggregate device created");

    let mut ctx = Ctx {
        frames: std::sync::atomic::AtomicU64::new(0),
        running: std::sync::atomic::AtomicBool::new(true),
        last_report: std::sync::atomic::AtomicU64::new(0),
        peak: 0.0,
        sample_rate,
    };
    let proc_id = agg_device
        .create_io_proc_id(on_audio, Some(&mut ctx))
        .map_err(|e| format!("create_io_proc_id: {}", e.0))?;
    // Must stay alive for the whole window: dropping it stops the device, which
    // looks exactly like a hang with zero samples (docs/AUDIO-TAP.md 3d).
    let started = ca::device_start(agg_device, Some(proc_id))
        .map_err(|e| format!("device_start: {}", e.0))?;
    // `agg_device` is moved into `device_start`; the StartedDevice is the handle
    // that keeps the IO running and dropping it is the teardown.

    println!("  IO running — {seconds}s window");
    std::thread::sleep(Duration::from_secs(seconds));
    ctx.running.store(false, std::sync::atomic::Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(200));

    let n = ctx.frames.load(std::sync::atomic::Ordering::Relaxed);
    drop(started);
    // The tap guard destroys the tap on drop; this makes the order explicit for
    // anyone reading the teardown.
    drop(tap);
    println!("  teardown: dropped StartedDevice and destroyed the tap");
    Ok(n)
}

// ---------------------------------------------------------------------------

fn pids_from_env() -> Vec<i32> {
    match std::env::var("TRAK_PIDS") {
        Ok(s) => s
            .split(',')
            .filter_map(|p| p.trim().parse::<i32>().ok())
            .collect(),
        Err(_) => ns::RunningApp::with_bundle_id(&ns::String::with_str("com.spotify.client"))
            .iter()
            .map(|a| a.pid())
            .collect(),
    }
}

/// Print what the runtime thinks is going on, before any conclusion depends on it.
fn preamble() {
    println!("trak tap spike (objc2 bypass) — TODO 1.5");
    println!("build target: {}", std::env::consts::ARCH);
    println!();

    let real = AnyClass::get(c"CATapDescription");
    let cidre_cls = ca::TapDesc::cls() as *const _ as *const AnyClass;
    println!("class resolution (this is the first thing that can be wrong):");
    match real {
        Some(c) => {
            println!(
                "  objc_getClass(\"CATapDescription\") -> {:p}   real class",
                c
            );
            // `AnyClass::name()` returns a C string owned by the runtime.
            // (`name` is an *instance* method on CATapDescription; there is no
            // class method, and msg_send'ing one panics under objc2's checks.)
            let name = c.name();
            println!("    name = {:?}", name);
        }
        None => println!("  objc_getClass(\"CATapDescription\") -> nil   !! CoreAudio did not load"),
    }
    println!(
        "  cidre's TapDesc::cls()      -> {:p}{}",
        cidre_cls,
        if Some(cidre_cls) == real.map(|c| c as *const _ as *const AnyClass) {
            "   == CATapDescription"
        } else {
            "   != CATapDescription  (cidre registered a stand-in class)"
        }
    );
    println!(
        "  the real class implements initMonoMixdownOfProcesses: {}",
        real.map(|c| c.responds_to(sel!(initMonoMixdownOfProcesses:))).unwrap_or(false),
    );
    println!();
}

fn main() {
    preamble();

    let shapes: Vec<Shape> = match std::env::var("TRAK_TAP_MODE") {
        Err(_) => vec![
            Shape::MonoMixdown,
            Shape::StereoMixdown,
            Shape::MonoGlobalExclude,
            Shape::StereoGlobalExclude,
        ],
        Ok(s) if s == "all" => vec![
            Shape::MonoMixdown,
            Shape::StereoMixdown,
            Shape::MonoGlobalExclude,
            Shape::StereoGlobalExclude,
        ],
        Ok(s) => match Shape::parse(&s) {
            Some(sh) => vec![sh],
            None => {
                eprintln!("unknown TRAK_TAP_MODE {s}");
                std::process::exit(2);
            }
        },
    };

    let kinds: Vec<ListKind> = match std::env::var("TRAK_LIST") {
        Err(_) => vec![ListKind::Empty, ListKind::Pids, ListKind::Objects],
        Ok(s) if s == "all" => vec![ListKind::Empty, ListKind::Pids, ListKind::Objects],
        Ok(s) => match ListKind::parse(&s) {
            Some(k) => vec![k],
            None => {
                eprintln!("unknown TRAK_LIST {s} (empty | pids | objects | all)");
                std::process::exit(2);
            }
        },
    };

    let pids = pids_from_env();
    println!("pids under test: {pids:?}");

    // The translation that the header asks for and the earlier spike skipped.
    let mut objects = Vec::new();
    for pid in &pids {
        match process_object_for_pid(*pid) {
            Ok(obj) if obj != 0 => {
                println!("  pid {pid} -> process object AudioObjectID 0x{obj:08X} ({obj})");
                objects.push(obj as i64);
            }
            Ok(_) => println!(
                "  pid {pid} -> kAudioObjectUnknown: this pid is not a Core Audio client, so it has no process object"
            ),
            Err(e) => println!("  pid {pid} -> translate failed {}", fourcc(e)),
        }
    }
    println!();

    // Reference data: what the pids look like next to the AudioObjectIDs they
    // have to be turned into. A reader should be able to see at a glance why a pid
    // is not an AudioObjectID.
    match process_objects() {
        Ok(list) => {
            let pairs: Vec<String> = list
                .iter()
                .filter_map(|o| process_pid(*o).map(|p| format!("0x{o:08X}={p}")))
                .collect();
            println!(
                "process object list ({} entries): {}",
                list.len(),
                if pairs.len() > 8 {
                    format!("{} ...", pairs[..8].join(", "))
                } else {
                    pairs.join(", ")
                }
            );
            println!();
        }
        Err(e) => println!("could not read the process object list: {}\n", fourcc(e)),
    }

    let capture_secs: u64 = std::env::var("TRAK_CAPTURE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let do_capture = std::env::var("TRAK_CAPTURE").is_ok();

    let mut wins: Vec<String> = Vec::new();
    for shape in &shapes {
        for kind in &kinds {
            let numbers: Vec<i64> = match kind {
                ListKind::Empty => Vec::new(),
                ListKind::Pids => pids.iter().map(|p| *p as i64).collect(),
                ListKind::Objects => objects.clone(),
            };
            println!(
                "--- {} / {} ({}) ---",
                shape.selector(),
                match kind {
                    ListKind::Empty => "empty list",
                    ListKind::Pids => "pids",
                    ListKind::Objects => "process AudioObjectIDs",
                },
                shape.describe()
            );

            let Some(desc) = make_tap_desc(*shape, &numbers) else {
                println!("  RESULT: initialiser returned nil\n");
                continue;
            };
            dump_tap_desc(&*desc);

            // The verdict for this cell: does macOS accept the description?
            match as_cidre_tap_desc(&*desc).create_process_tap() {
                Ok(tap) => {
                    let uid = tap.uid().map(|u| u.to_string()).unwrap_or_default();
                    let asbd = tap
                        .asbd()
                        .map(|a| {
                            format!(
                                "{} Hz, {} ch, {} bit, flags={:?}",
                                a.sample_rate,
                                a.channels_per_frame,
                                a.bits_per_channel,
                                a.format_flags
                            )
                        })
                        .unwrap_or_else(|e| format!("asbd failed {}", e.0));
                    println!("  RESULT: ACCEPTED — tap uid {uid}");
                    println!("          asbd {asbd}");
                    drop(tap);
                    wins.push(format!("{} / {kind:?}", shape.selector()));
                }
                Err(e) => println!("  RESULT: REJECTED — {}  {}", e.0.get(), fourcc(e.0.get() as i32)),
            }
            println!();
        }
    }

    // Optional end-to-end capture on a single cell, to prove "accepted" means
    // audio and not just a non-nil object.
    if do_capture {
        let Some(shape) = shapes.first() else { return };
        let numbers: Vec<i64> = match kinds.first().copied().unwrap_or(ListKind::Objects) {
            ListKind::Empty => Vec::new(),
            ListKind::Pids => pids.iter().map(|p| *p as i64).collect(),
            ListKind::Objects => objects.clone(),
        };
        println!("=== capture run: {} / {} ===", shape.selector(), numbers.len());
        match make_tap_desc(*shape, &numbers) {
            Some(desc) => match capture(&*desc, capture_secs) {
                Ok(n) => {
                    println!("\n--- {n} float samples in {capture_secs}s ({:.1}/s) ---",
                        n as f64 / capture_secs as f64);
                    if n == 0 {
                        println!("NOTHING ARRIVED: the tap was accepted but delivered no audio.");
                    }
                }
                Err(e) => println!("capture failed: {e}"),
            },
            None => println!("capture skipped: the initialiser returned nil"),
        }
    }

    println!();
    if wins.is_empty() {
        println!("summary: no tap description was accepted.");
    } else {
        println!("summary: accepted:");
        for w in &wins {
            println!("  {w}");
        }
    }
}