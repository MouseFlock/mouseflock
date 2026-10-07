//! Tests that install real hooks and inject real input. They need a
//! logged-in desktop and briefly type F24, scroll and move the cursor, so
//! they never run by default:
//!
//! ```text
//! cargo test -p win32-input -- --ignored --test-threads=1
//! ```
//!
//! Keep hands off the keyboard and mouse while they run.

use std::sync::mpsc::Receiver;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use model::{CaptureMode, InputAction, InputEvent, Key, Origin, Point};
use win32_input::{Capture, Captured, Options, StartError, start};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

const F24: Key = Key(0x73);
const VK_F24: i32 = 0x87;
const WAIT: Duration = Duration::from_secs(2);

/// One capture per process: tests take turns.
fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// The first event within `within` that `want` accepts.
fn wait_for(
    rx: &Receiver<Captured>,
    within: Duration,
    want: impl Fn(&Captured) -> bool,
) -> Option<Captured> {
    let end = Instant::now() + within;
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(c) if want(&c) => return Some(c),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

fn is_injected_key(c: &Captured, key: Key, down: bool) -> bool {
    matches!(c, Captured::Input { input: InputEvent::Key { key: k, down: d }, origin: Origin::Injected, .. }
        if *k == key && *d == down)
}

fn press(capture: &Capture, rx: &Receiver<Captured>, key: Key, down: bool) {
    capture.inject(InputAction::Key { key, down }).unwrap();
    assert!(
        wait_for(rx, WAIT, |c| is_injected_key(c, key, down)).is_some(),
        "no injected {key:?} down={down}"
    );
}

fn cursor() -> Point {
    let mut p = POINT::default();
    // SAFETY: `p` is a valid, writable POINT.
    unsafe { GetCursorPos(&mut p) }.unwrap();
    Point::new(p.x, p.y)
}

fn f24_down() -> bool {
    // SAFETY: plain call with a virtual-key code.
    (unsafe { GetAsyncKeyState(VK_F24) } as u16) & 0x8000 != 0
}

/// True if `f24_down()` equals `want` within 500 ms: the OS updates key
/// state after the hooks have run.
fn f24_becomes(want: bool) -> bool {
    let end = Instant::now() + Duration::from_millis(500);
    while Instant::now() < end {
        if f24_down() == want {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
#[ignore = "needs a desktop"]
fn second_start_fails() {
    let _s = serial();
    let (_capture, _rx) = start(Options::default()).unwrap();
    assert!(matches!(
        start(Options::default()),
        Err(StartError::AlreadyRunning)
    ));
}

#[test]
#[ignore = "needs a desktop"]
fn start_after_drop_succeeds() {
    let _s = serial();
    drop(start(Options::default()).unwrap());
    let (_capture, _rx) = start(Options::default()).unwrap();
}

#[test]
#[ignore = "needs a desktop"]
fn injected_key_is_reported_as_injected() {
    let _s = serial();
    let (capture, rx) = start(Options::default()).unwrap();
    press(&capture, &rx, F24, true);
    press(&capture, &rx, F24, false);
}

#[test]
#[ignore = "needs a desktop"]
fn injected_key_passes_while_withholding_all() {
    let _s = serial();
    let (capture, rx) = start(Options::default()).unwrap();
    capture.set_mode(CaptureMode::WithholdAll);
    press(&capture, &rx, F24, true);
    let reached = f24_becomes(true);
    press(&capture, &rx, F24, false);
    assert!(reached, "the injected F24 did not reach the OS");
    assert!(f24_becomes(false));
}

#[test]
#[ignore = "needs a desktop"]
fn injected_wheel_is_reported() {
    let _s = serial();
    let (capture, rx) = start(Options::default()).unwrap();
    for dy in [120, -120] {
        capture.inject(InputAction::Wheel { dx: 0, dy }).unwrap();
        let seen = wait_for(
            &rx,
            WAIT,
            |c| matches!(c, Captured::Input { input: InputEvent::Wheel { dx: 0, dy: d }, origin: Origin::Injected, .. } if *d == dy),
        );
        assert!(seen.is_some(), "no injected wheel {dy}");
    }
}

#[test]
#[ignore = "needs a desktop"]
fn injected_motion_is_reported_as_injected() {
    let _s = serial();
    let (capture, rx) = start(Options::default()).unwrap();
    for dx in [7, -7] {
        capture.inject(InputAction::Motion { dx, dy: 0 }).unwrap();
        let seen = wait_for(
            &rx,
            WAIT,
            |c| matches!(c, Captured::Input { input: InputEvent::Motion { dx: x, dy: 0, .. }, origin: Origin::Injected, .. } if *x == dx),
        );
        assert!(seen.is_some(), "no injected motion {dx}");
    }
}

#[test]
#[ignore = "needs a desktop"]
fn warp_moves_the_cursor_without_motion() {
    let _s = serial();
    let (capture, rx) = start(Options::default()).unwrap();
    let Some(Captured::Screen(screen, _)) =
        wait_for(&rx, WAIT, |c| matches!(c, Captured::Screen(..)))
    else {
        panic!("no screen report");
    };
    let r = screen.monitors[0].rect;
    let target = Point::new(r.x + r.w / 2, r.y + r.h / 2);
    let before = cursor();

    capture.warp(target).unwrap();
    let after = cursor();
    let motion = wait_for(&rx, Duration::from_millis(200), |c| {
        matches!(
            c,
            Captured::Input {
                input: InputEvent::Motion { .. },
                ..
            }
        )
    });
    capture.warp(before).unwrap();

    assert_eq!(after, target);
    assert_eq!(motion, None, "a warp was reported as motion");
}

#[test]
#[ignore = "needs a desktop"]
fn first_screen_contains_the_cursor() {
    let _s = serial();
    let (_capture, rx) = start(Options::default()).unwrap();
    let Some(Captured::Screen(screen, details)) =
        wait_for(&rx, WAIT, |c| matches!(c, Captured::Screen(..)))
    else {
        panic!("no screen report");
    };
    assert!(!screen.monitors.is_empty());
    assert_eq!(screen.monitors.len(), details.len());
    assert!(screen.monitors.iter().all(|m| m.dpi >= 96));
    let p = cursor();
    assert!(
        screen.monitor_at(p).is_some(),
        "cursor {p:?} outside {screen:?}"
    );
}

#[test]
#[ignore = "needs a desktop"]
fn unchanged_screen_is_not_reported_again() {
    let _s = serial();
    let (_capture, rx) = start(Options::default()).unwrap();
    assert!(wait_for(&rx, WAIT, |c| matches!(c, Captured::Screen(..))).is_some());
    let again = wait_for(&rx, Duration::from_secs(5), |c| {
        matches!(c, Captured::Screen(..))
    });
    assert_eq!(again, None);
}

/// The reports that are not input, in order, for `within`.
fn reports(rx: &Receiver<Captured>, within: Duration) -> Vec<Captured> {
    let end = Instant::now() + within;
    let mut out = Vec::new();
    while let Ok(c) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
        if !matches!(c, Captured::Input { .. }) {
            out.push(c);
        }
    }
    out
}

#[test]
#[ignore = "needs a desktop"]
fn input_desktop_follows_the_first_screen() {
    let _s = serial();
    let (_capture, rx) = start(Options {
        report_input_desktop: true,
    })
    .unwrap();
    let got = reports(&rx, Duration::from_millis(500));
    // Tests run on the user's desktop, which has the input.
    assert!(
        matches!(
            got.as_slice(),
            [Captured::Screen(..), Captured::InputDesktop(true)]
        ),
        "{got:?}"
    );
}

#[test]
#[ignore = "needs a desktop"]
fn no_input_desktop_reports_unless_asked() {
    let _s = serial();
    let (_capture, rx) = start(Options::default()).unwrap();
    let got = reports(&rx, Duration::from_millis(500));
    assert!(matches!(got.as_slice(), [Captured::Screen(..)]), "{got:?}");
}
