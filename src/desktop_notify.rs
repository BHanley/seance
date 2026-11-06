//! Desktop notifications for attention routing (needs-human / ask).
//!
//! Fire-and-forget `notify-send` on Linux. On macOS, `terminal-notifier` when
//! installed (a click brings Seance forward and selects the pane), else
//! `osascript` (a click opens Script Editor, which owns those notifications).
//! Silent no-op if the binary is missing.

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static LAST_MS: AtomicU64 = AtomicU64::new(0);

/// Dedup window so a thrashing agent can't spam the notification daemon.
const MIN_GAP: Duration = Duration::from_secs(4);

pub fn notify(summary: &str, body: &str) {
    post(&LAST_MS, None, summary, body, None);
}

/// Pane alerts (OSC 9/777, finished commands). Own dedup slot, so an alert
/// can't swallow a needs-human or ask notification right behind it.
pub fn notify_alert(pane: &str, summary: &str, body: &str) {
    static LAST_ALERT_MS: AtomicU64 = AtomicU64::new(0);
    post(&LAST_ALERT_MS, Some(pane), summary, body, None);
}

/// A Claude finished working. Dedup is per pane, not global: two Claudes
/// finishing together should both show.
pub fn notify_done(pane: &str, summary: &str, body: &str) {
    static LAST: std::sync::Mutex<Option<std::collections::HashMap<String, u64>>> =
        std::sync::Mutex::new(None);
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    {
        let mut guard = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let map = guard.get_or_insert_with(Default::default);
        if map
            .get(pane)
            .is_some_and(|prev| wall.saturating_sub(*prev) < MIN_GAP.as_millis() as u64)
        {
            return;
        }
        map.insert(pane.to_string(), wall);
    }
    post(&AtomicU64::new(0), Some(pane), summary, body, None);
}

/// `pane`, when known, is selected on click (macOS with terminal-notifier).
/// `sound` is a macOS sound name for the banner (`default` = the system's);
/// None is silent. Linux banners never carry one.
fn post(last_ms: &AtomicU64, pane: Option<&str>, summary: &str, body: &str, sound: Option<&str>) {
    if !crate::term_config::get().seance_desktop_notifications {
        return;
    }
    let now = Instant::now();
    let now_ms = now.elapsed().as_millis() as u64; // wrong baseline — use wall clock
    let _ = now_ms;
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let prev = last_ms.load(Ordering::Relaxed);
    if wall.saturating_sub(prev) < MIN_GAP.as_millis() as u64 {
        return;
    }
    last_ms.store(wall, Ordering::Relaxed);

    let summary = summary.to_string();
    let body = body.to_string();
    let pane = pane.map(str::to_string);
    let sound = sound.map(str::to_string);
    std::thread::spawn(move || {
        if cfg!(target_os = "macos") {
            if mac_terminal_notifier(pane.as_deref(), &summary, &body, sound.as_deref()) {
                return;
            }
            // Quotes are escaped for the AppleScript string literals.
            let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
            // AppleScript has no "default" sound name; Glass is the closest.
            let with_sound = match sound.as_deref() {
                Some("default") => " sound name \"Glass\"".to_string(),
                Some(name) => format!(" sound name \"{}\"", esc(name)),
                None => String::new(),
            };
            let script = format!(
                "display notification \"{}\" with title \"{}\"{with_sound}",
                esc(&body),
                esc(&summary)
            );
            let _ = Command::new("osascript")
                .args(["-e", &script])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            return;
        }
        let _ = Command::new("notify-send")
            .args([
                "--app-name=seance",
                "--urgency=normal",
                "--expire-time=12000",
                &summary,
                &body,
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// Apps opened from Finder don't get Homebrew on PATH, so check its
/// prefixes first.
fn terminal_notifier() -> Option<std::path::PathBuf> {
    ["/opt/homebrew/bin", "/usr/local/bin"]
        .iter()
        .map(|d| std::path::Path::new(d).join("terminal-notifier"))
        .find(|p| p.exists())
}

/// Banner groups posted since launch and not yet removed.
static POSTED: std::sync::Mutex<std::collections::BTreeSet<String>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// The human read `pane`: take its banner out of Notification Center.
/// macOS with terminal-notifier only.
pub fn clear_pane(pane: &str) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let group = format!("seance-{pane}");
    // Only groups with a banner up: this runs on every change of pane.
    if !POSTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&group)
    {
        return;
    }
    let Some(bin) = terminal_notifier() else {
        return;
    };
    std::thread::spawn(move || {
        let _ = Command::new(bin)
            .args(["-remove", &group])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// False when terminal-notifier isn't installed.
fn mac_terminal_notifier(
    pane: Option<&str>,
    summary: &str,
    body: &str,
    sound: Option<&str>,
) -> bool {
    let Some(bin) = terminal_notifier() else {
        return false;
    };
    let mut cmd = Command::new(bin);
    cmd.args(["-title", summary, "-message", body]);
    // One banner per pane: a new one replaces the last, and reading the pane
    // takes it away (`clear_pane`).
    if let Some(pane) = pane {
        let group = format!("seance-{pane}");
        cmd.args(["-group", &group]);
        POSTED
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(group);
    }
    if let Some(sound) = sound {
        cmd.args(["-sound", sound]);
    }
    // A click brings Seance forward (its bundle id from bundle-macos.sh) and
    // selects the pane. No -sender: it disables both, so the icon stays
    // terminal-notifier's.
    cmd.args(["-activate", "xyz.ham.seance"]);
    if let Ok(exe) = std::env::current_exe() {
        if let Some(pane) = pane {
            let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
            let run = format!(
                "{} ctl --all select {}",
                quote(&exe.to_string_lossy()),
                quote(pane)
            );
            cmd.args(["-execute", &run]);
        }
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Play `bell-audio-path` from `~/.config/seance/terminal.conf` when
/// `bell-features` includes `audio`.
/// Fire-and-forget like [`notify`]. On macOS it plays through the sound
/// effects device from Sound settings (`afplay` when that fails); on Linux
/// `pw-play`, falling back to `paplay`. A burst of bells inside a second plays once.
pub fn bell_sound() {
    static LAST_BELL_MS: AtomicU64 = AtomicU64::new(0);
    let cfg = crate::term_config::get();
    let Some(path) = cfg.bell_audio_path.clone().filter(|_| cfg.bell_audio) else {
        return;
    };
    play(&LAST_BELL_MS, path, cfg.bell_audio_volume);
}

/// `seance-done-sound`: a circle lead finished while you were looking elsewhere.
/// Several finishing within a second play once.
pub fn done_sound() {
    static LAST_DONE_MS: AtomicU64 = AtomicU64::new(0);
    let cfg = crate::term_config::get();
    let Some(path) = cfg.seance_done_sound.clone() else {
        return;
    };
    play(&LAST_DONE_MS, path, cfg.seance_done_sound_volume);
}

/// Play a sound file at `volume` (0 to 1), at most once a second per `last`.
fn play(last: &AtomicU64, path: std::path::PathBuf, volume: f32) {
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    if wall.saturating_sub(last.swap(wall, Ordering::Relaxed)) < 1_000 {
        return;
    }
    std::thread::spawn(move || {
        let quiet = |c: &mut Command| {
            c.stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        let volume_arg = volume.to_string();
        if cfg!(target_os = "macos") {
            // afplay can't pick a device, so it is only the fallback.
            if !mac_system_play(&path, volume) {
                quiet(Command::new("afplay").args(["-v", &volume_arg]).arg(&path));
            }
        } else if !quiet(
            Command::new("pw-play")
                .args(["--volume", &volume_arg])
                .arg(&path),
        ) {
            quiet(Command::new("paplay").arg(&path));
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn mac_system_play(_path: &std::path::Path, _volume: f32) -> bool {
    false
}

/// Plays on Sound settings' "Play sound effects through" device. False when
/// that device can't be resolved or NSSound refuses the file.
#[cfg(target_os = "macos")]
fn mac_system_play(path: &std::path::Path, volume: f32) -> bool {
    match sound_backend(mac_audio::default_system_output().map(|d| d.uid)) {
        SoundBackend::Device(uid) => mac_audio::play_on_device(path, clamp_volume(volume), &uid),
        SoundBackend::Afplay => false,
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, PartialEq)]
enum SoundBackend {
    Device(String),
    Afplay,
}

#[cfg(any(target_os = "macos", test))]
fn sound_backend(system_uid: Option<String>) -> SoundBackend {
    match system_uid {
        Some(uid) if !uid.is_empty() => SoundBackend::Device(uid),
        _ => SoundBackend::Afplay,
    }
}

/// NaN parses as a valid f32 and survives `clamp`, so it maps to full volume.
#[cfg(any(target_os = "macos", test))]
fn clamp_volume(volume: f32) -> f32 {
    if volume.is_nan() {
        1.0
    } else {
        volume.clamp(0.0, 1.0)
    }
}

/// Raw CoreAudio and Objective-C calls: the few needed here don't justify a
/// direct objc or core-foundation dependency.
#[cfg(target_os = "macos")]
mod mac_audio {
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::time::{Duration, Instant};

    type Id = *mut c_void;
    type Sel = *const c_void;
    type CFStringRef = *const c_void;

    #[repr(C)]
    struct PropertyAddress {
        selector: u32,
        scope: u32,
        element: u32,
    }

    const fn fourcc(code: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*code)
    }

    const SYSTEM_OBJECT: u32 = 1;
    #[cfg(test)]
    const DEFAULT_OUTPUT: u32 = fourcc(b"dOut");
    const DEFAULT_SYSTEM_OUTPUT: u32 = fourcc(b"sOut");
    const DEVICE_UID: u32 = fourcc(b"uid ");
    const OBJECT_NAME: u32 = fourcc(b"lnam");
    const SCOPE_GLOBAL: u32 = fourcc(b"glob");
    const ELEMENT_MAIN: u32 = 0;
    const CF_UTF8: u32 = 0x0800_0100;

    /// Longest a sound is kept alive for; done and bell sounds are short.
    const MAX_PLAY: Duration = Duration::from_secs(10);

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectGetPropertyData(
            object: u32,
            address: *const PropertyAddress,
            qualifier_size: u32,
            qualifier: *const c_void,
            data_size: *mut u32,
            data: *mut c_void,
        ) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFRelease(cf: *const c_void);
    }

    // AppKit holds NSSound; linking it keeps objc_getClass working in tests.
    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_msgSend();
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }

    pub struct Device {
        pub uid: String,
        #[cfg_attr(not(test), allow(dead_code))]
        pub name: String,
    }

    /// Sound settings' "Play sound effects through" device.
    pub fn default_system_output() -> Option<Device> {
        device(DEFAULT_SYSTEM_OUTPUT)
    }

    /// The device music and afplay use.
    #[cfg(test)]
    pub fn default_output() -> Option<Device> {
        device(DEFAULT_OUTPUT)
    }

    fn device(which: u32) -> Option<Device> {
        let mut id: u32 = 0;
        if !get_property(SYSTEM_OBJECT, which, &mut id) || id == 0 {
            return None;
        }
        Some(Device {
            uid: string_property(id, DEVICE_UID)?,
            name: string_property(id, OBJECT_NAME).unwrap_or_default(),
        })
    }

    fn get_property<T>(object: u32, selector: u32, out: &mut T) -> bool {
        let address = PropertyAddress {
            selector,
            scope: SCOPE_GLOBAL,
            element: ELEMENT_MAIN,
        };
        let mut size = std::mem::size_of::<T>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                object,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                out as *mut T as *mut c_void,
            )
        };
        status == 0 && size as usize == std::mem::size_of::<T>()
    }

    /// The property's CFString comes back retained; it is released here.
    fn string_property(object: u32, selector: u32) -> Option<String> {
        let mut cf: CFStringRef = std::ptr::null();
        if !get_property(object, selector, &mut cf) || cf.is_null() {
            return None;
        }
        let mut buf = [0 as c_char; 512];
        let ok = unsafe { CFStringGetCString(cf, buf.as_mut_ptr(), buf.len() as isize, CF_UTF8) };
        unsafe { CFRelease(cf) };
        (ok != 0).then(|| {
            unsafe { CStr::from_ptr(buf.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        })
    }

    fn sel(name: &str) -> Sel {
        let name = CString::new(name).expect("selector has no NUL");
        unsafe { sel_registerName(name.as_ptr()) }
    }

    /// objc_msgSend must be called through the exact signature of each method.
    macro_rules! send {
        ($obj:expr, $sel:expr $(, $arg:expr => $ty:ty)* ; $ret:ty) => {{
            let f: unsafe extern "C" fn(Id, Sel $(, $ty)*) -> $ret =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            f($obj, sel($sel) $(, $arg)*)
        }};
    }

    /// Autoreleased; needs a pool.
    unsafe fn ns_string(s: &str) -> Option<Id> {
        let c = CString::new(s).ok()?;
        let class = objc_getClass(c"NSString".as_ptr());
        if class.is_null() {
            return None;
        }
        let ns = send!(class, "stringWithUTF8String:", c.as_ptr() => *const c_char; Id);
        (!ns.is_null()).then_some(ns)
    }

    /// A retained NSSound for `path`, routed to `uid`, not yet playing.
    /// The caller releases it. Must run inside an autorelease pool.
    unsafe fn routed_sound(path: &std::path::Path, volume: f32, uid: &str) -> Option<Id> {
        let class = objc_getClass(c"NSSound".as_ptr());
        if class.is_null() {
            return None;
        }
        let path = ns_string(path.to_str()?)?;
        let uid = ns_string(uid)?;
        let alloc = send!(class, "alloc"; Id);
        let sound =
            send!(alloc, "initWithContentsOfFile:byReference:", path => Id, true => bool; Id);
        if sound.is_null() {
            return None;
        }
        send!(sound, "setVolume:", volume => f32; ());
        send!(sound, "setPlaybackDeviceIdentifier:", uid => Id; ());
        Some(sound)
    }

    /// Blocks the calling thread until the sound ends (at most [`MAX_PLAY`])
    /// so the NSSound stays alive. False when NSSound can't play the file.
    pub fn play_on_device(path: &std::path::Path, volume: f32, uid: &str) -> bool {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let played = match routed_sound(path, volume, uid) {
                Some(sound) => {
                    let ok = send!(sound, "play"; bool);
                    if ok {
                        let start = Instant::now();
                        while send!(sound, "isPlaying"; bool) && start.elapsed() < MAX_PLAY {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        send!(sound, "stop"; bool);
                    }
                    send!(sound, "release"; ());
                    ok
                }
                None => false,
            };
            objc_autoreleasePoolPop(pool);
            played
        }
    }

    /// The device identifier an NSSound reports after routing. Never plays.
    #[cfg(test)]
    pub fn routed_identifier(path: &std::path::Path, uid: &str) -> Option<String> {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let out = routed_sound(path, 0.5, uid).and_then(|sound| {
                let id = send!(sound, "playbackDeviceIdentifier"; Id);
                let s = (!id.is_null()).then(|| {
                    let c = send!(id, "UTF8String"; *const c_char);
                    CStr::from_ptr(c).to_string_lossy().into_owned()
                });
                send!(sound, "release"; ());
                s
            });
            objc_autoreleasePoolPop(pool);
            out
        }
    }
}

/// `label` is what the human calls the pane ("circle · name"); `pane` stays
/// the slug the click jumps to.
pub fn needs_human(pane: &str, label: &str, note: Option<&str>) {
    let body = match note {
        Some(n) if !n.is_empty() => format!("{label}: {n}"),
        _ => format!("{label} needs you"),
    };
    let sound = crate::term_config::get().seance_needs_sound.as_deref();
    post(&LAST_MS, Some(pane), "seance · needs human", &body, sound);
}

pub fn ask(from: &str, question: &str) {
    let q = if question.len() > 160 {
        // Cut on a char boundary: a byte slice through an emoji panics.
        let mut end = 160;
        while !question.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &question[..end])
    } else {
        question.to_string()
    };
    let sound = crate::term_config::get().seance_needs_sound.as_deref();
    post(
        &LAST_MS,
        None,
        "seance · agent asks",
        &format!("{from}: {q}"),
        sound,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_device_uid_picks_nssound() {
        assert_eq!(
            sound_backend(Some("BuiltInSpeakerDevice".into())),
            SoundBackend::Device("BuiltInSpeakerDevice".into())
        );
    }

    #[test]
    fn missing_or_empty_uid_falls_back_to_afplay() {
        assert_eq!(sound_backend(None), SoundBackend::Afplay);
        assert_eq!(sound_backend(Some(String::new())), SoundBackend::Afplay);
    }

    #[test]
    fn volume_is_clamped_and_nan_is_full() {
        assert_eq!(clamp_volume(0.6), 0.6);
        assert_eq!(clamp_volume(-1.0), 0.0);
        assert_eq!(clamp_volume(3.0), 1.0);
        assert_eq!(clamp_volume(f32::NAN), 1.0);
    }

    /// Prints the two default output devices. Plays nothing.
    /// `cargo test print_output_devices -- --ignored --nocapture`
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn print_output_devices() {
        let show = |d: Option<mac_audio::Device>| {
            d.map(|d| format!("{} ({})", d.name, d.uid))
                .unwrap_or_else(|| "none".into())
        };
        println!(
            "system output: {}",
            show(mac_audio::default_system_output())
        );
        println!("default output: {}", show(mac_audio::default_output()));
    }

    /// Routing sticks on the NSSound without ever calling play.
    #[cfg(target_os = "macos")]
    #[test]
    fn nssound_takes_the_system_device() {
        let Some(device) = mac_audio::default_system_output() else {
            eprintln!("no system output device; skipped");
            return;
        };
        let glass = std::path::Path::new("/System/Library/Sounds/Glass.aiff");
        assert_eq!(
            mac_audio::routed_identifier(glass, &device.uid),
            Some(device.uid)
        );
    }
}
