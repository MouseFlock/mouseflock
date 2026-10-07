//! Starting and stopping capture; the input thread's hooks and Raw Input.

use std::cell::RefCell;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread::JoinHandle;

use model::{CaptureMode, InputAction, InputEvent, Origin, Point};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    GetThreadDpiAwarenessContext, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT,
    RIDEV_INPUTSINK, RIM_TYPEMOUSE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_INJECTED, LLKHF_UP, LLMHF_INJECTED, MSG,
    MSLLHOOKSTRUCT, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_INPUT, WM_QUIT, WS_EX_TOOLWINDOW,
};
use windows::core::PCWSTR;

use crate::decide::{self, Kind, MouseMessage, TAG};
use crate::keys::{self, KeyKind};
use crate::{Captured, InjectError, WarpError, inject, now, screen};

/// True while a [`Capture`] exists; hooks have no user data, so their state
/// is per process.
static RUNNING: AtomicBool = AtomicBool::new(false);
/// The current [`CaptureMode`], read by the hooks.
static MODE: AtomicU8 = AtomicU8::new(PASS_ALL);
/// Whether the last move the mouse hook saw, other than DeskHop's own, was
/// injected (design D4).
static LAST_MOVE_INJECTED: AtomicBool = AtomicBool::new(false);

const PASS_ALL: u8 = 0;
const WITHHOLD_ALL: u8 = 1;
const WITHHOLD_MOUSE: u8 = 2;

thread_local! {
    /// Where the input thread's hooks and Raw Input handler send events.
    static SINK: RefCell<Option<Sender<Captured>>> = const { RefCell::new(None) };
}

/// Why [`start`] failed.
#[derive(Debug)]
pub enum StartError {
    /// Another [`Capture`] is running in this process.
    AlreadyRunning,
    /// The process could not be made per-monitor DPI aware (v2), so
    /// coordinates would not be physical pixels.
    DpiAwareness,
    /// A Win32 call failed while setting up.
    Setup(windows::core::Error),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => f.write_str("input capture is already running in this process"),
            Self::DpiAwareness => f.write_str("the process is not per-monitor DPI aware (v2)"),
            Self::Setup(e) => write!(f, "setting up input capture failed: {e}"),
        }
    }
}

impl std::error::Error for StartError {}

/// A running capture. Dropping it returns to [`CaptureMode::PassAll`],
/// removes the hooks and stops both threads.
pub struct Capture {
    workers: Vec<Worker>,
}

/// What [`start`] reports besides input and the screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Report [`Captured::InputDesktop`]: once right after the first
    /// [`Captured::Screen`], then within 100 ms of each change. For the
    /// winlogon agent.
    pub report_input_desktop: bool,
}

/// Installs the hooks, registers for Raw Input, and starts reporting.
///
/// The first [`Captured::Screen`] arrives promptly after start. Fails if a
/// capture is already running in this process.
pub fn start(options: Options) -> Result<(Capture, Receiver<Captured>), StartError> {
    if RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(StartError::AlreadyRunning);
    }
    MODE.store(PASS_ALL, Ordering::Release);
    let mut capture = Capture {
        workers: Vec::new(),
    };
    dpi_aware()?;
    let (tx, rx) = mpsc::channel();
    capture
        .workers
        .push(Worker::spawn("deskhop-input", input_thread, tx.clone())?);
    let report_desktop = options.report_input_desktop;
    capture.workers.push(Worker::spawn(
        "deskhop-screen",
        move |tx, ready| screen::thread(tx, ready, report_desktop),
        tx,
    )?);
    Ok((capture, rx))
}

impl Capture {
    /// Sets which physical input the hooks keep from the OS. Applies to
    /// every event delivered after this returns.
    pub fn set_mode(&self, mode: CaptureMode) {
        let m = match mode {
            CaptureMode::PassAll => PASS_ALL,
            CaptureMode::WithholdAll => WITHHOLD_ALL,
            CaptureMode::WithholdMouse => WITHHOLD_MOUSE,
        };
        MODE.store(m, Ordering::Release);
    }

    /// Injects `action` into this machine's OS, tagged as DeskHop's own.
    pub fn inject(&self, action: InputAction) -> Result<(), InjectError> {
        inject::send(action)
    }

    /// Moves the cursor to `to`, in virtual-screen pixels. Not reported as
    /// motion.
    pub fn warp(&self, to: Point) -> Result<(), WarpError> {
        inject::warp(to)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        MODE.store(PASS_ALL, Ordering::Release);
        for w in &mut self.workers {
            w.stop();
        }
        RUNNING.store(false, Ordering::Release);
    }
}

/// Makes the process per-monitor DPI aware (v2), or confirms it already is.
fn dpi_aware() -> Result<(), StartError> {
    // SAFETY: plain calls with a predefined awareness context. Setting fails
    // harmlessly if the awareness was already set; the check decides.
    let ok = unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        AreDpiAwarenessContextsEqual(
            GetThreadDpiAwarenessContext(),
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
        .as_bool()
    };
    if ok {
        Ok(())
    } else {
        Err(StartError::DpiAwareness)
    }
}

/// A thread with a message loop, stopped by posting `WM_QUIT`.
struct Worker {
    thread_id: u32,
    handle: Option<JoinHandle<()>>,
}

/// What a worker thread reports once it is set up: its thread id, or why
/// setting up failed.
pub(crate) type Ready = SyncSender<windows::core::Result<u32>>;

impl Worker {
    fn spawn(
        name: &str,
        body: impl FnOnce(Sender<Captured>, Ready) + Send + 'static,
        tx: Sender<Captured>,
    ) -> Result<Worker, StartError> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let handle = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || body(tx, ready_tx))
            .map_err(|e| StartError::Setup(windows::core::Error::from(e)))?;
        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok(Worker {
                thread_id,
                handle: Some(handle),
            }),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(StartError::Setup(e))
            }
            Err(_) => {
                let _ = handle.join();
                Err(StartError::Setup(windows::core::Error::from_win32()))
            }
        }
    }

    fn stop(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        // SAFETY: posts a message with no pointers to a thread whose queue
        // exists (it was created before the thread reported ready).
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        let _ = handle.join();
    }
}

/// The current thread's id.
pub(crate) fn thread_id() -> u32 {
    // SAFETY: no arguments; cannot fail.
    unsafe { GetCurrentThreadId() }
}

/// A hidden top-level window of `class`: top-level so that it receives
/// broadcast messages, which message-only windows do not.
pub(crate) fn hidden_window(class: PCWSTR) -> windows::core::Result<HWND> {
    // SAFETY: `class` is a registered or system class name that outlives the
    // call; all other arguments are plain values or absent.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WINDOW_EX_STYLE(0),
            class,
            windows::core::w!("DeskHop"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
}

/// Runs a message loop until `WM_QUIT`, handing each message to `on`
/// before dispatching it.
pub(crate) fn message_loop(mut on: impl FnMut(&MSG)) {
    let mut msg = MSG::default();
    loop {
        // SAFETY: `msg` is a valid, writable MSG.
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        on(&msg);
        // SAFETY: `msg` was filled in by GetMessageW.
        unsafe { DispatchMessageW(&msg) };
    }
}

/// The input thread: both hooks and the Raw Input window, and nothing else.
fn input_thread(tx: Sender<Captured>, ready: Ready) {
    SINK.with(|s| *s.borrow_mut() = Some(tx));
    let setup = || -> windows::core::Result<_> {
        let hwnd = hidden_window(windows::core::w!("STATIC"))?;
        let device = RAWINPUTDEVICE {
            usUsagePage: 0x01, // Generic desktop
            usUsage: 0x02,     // Mouse
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };
        // SAFETY: the window belongs to this thread and lives until after
        // the hooks are removed below; the hook procedures are `extern
        // "system"` functions with the HOOKPROC signature.
        unsafe {
            windows::Win32::UI::Input::RegisterRawInputDevices(
                &[device],
                size_of::<RAWINPUTDEVICE>() as u32,
            )?;
            let instance = GetModuleHandleW(None)?;
            let keyboard = SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_hook),
                Some(instance.into()),
                0,
            )?;
            let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(instance.into()), 0)?;
            Ok((hwnd, keyboard, mouse))
        }
    };
    let (hwnd, keyboard, mouse) = match setup() {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(thread_id()));

    message_loop(|msg| {
        if msg.message == WM_INPUT {
            on_raw_input(msg.lParam);
        }
    });

    // SAFETY: the hooks and window were created on this thread above.
    unsafe {
        let _ = UnhookWindowsHookEx(keyboard);
        let _ = UnhookWindowsHookEx(mouse);
        let _ = DestroyWindow(hwnd);
    }
    SINK.with(|s| *s.borrow_mut() = None);
}

fn mode() -> CaptureMode {
    match MODE.load(Ordering::Acquire) {
        WITHHOLD_ALL => CaptureMode::WithholdAll,
        WITHHOLD_MOUSE => CaptureMode::WithholdMouse,
        _ => CaptureMode::PassAll,
    }
}

fn report(input: InputEvent, origin: Origin) {
    let event = Captured::Input {
        at: now(),
        input,
        origin,
    };
    SINK.with(|s| {
        if let Ok(s) = s.try_borrow()
            && let Some(tx) = s.as_ref()
        {
            let _ = tx.send(event);
        }
    });
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, lParam points to a KBDLLHOOKSTRUCT that is
        // valid for the duration of this call.
        let k = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if on_key(k) {
            return LRESULT(1);
        }
    }
    // SAFETY: passes this hook's arguments on unchanged.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Reports a key and returns true to withhold it.
fn on_key(k: &KBDLLHOOKSTRUCT) -> bool {
    let flags = k.flags;
    let origin = if flags.contains(LLKHF_INJECTED) {
        Origin::Injected
    } else {
        Origin::Physical
    };
    let kind = match keys::classify(k.scanCode, flags.contains(LLKHF_EXTENDED), k.vkCode) {
        KeyKind::Key(key) => {
            let down = !flags.contains(LLKHF_UP);
            report(InputEvent::Key { key, down }, origin);
            Kind::Key
        }
        KeyKind::FakeShift => Kind::FakeShift,
        KeyKind::Other => Kind::OtherKey,
    };
    decide::withhold(mode(), kind, origin)
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, lParam points to an MSLLHOOKSTRUCT that is
        // valid for the duration of this call.
        let m = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if on_mouse(wparam.0 as u32, m) {
            return LRESULT(1);
        }
    }
    // SAFETY: passes this hook's arguments on unchanged.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Reports a button or wheel event and returns true to withhold it.
fn on_mouse(message: u32, m: &MSLLHOOKSTRUCT) -> bool {
    let injected = m.flags & LLMHF_INJECTED != 0;
    let origin = if injected {
        Origin::Injected
    } else {
        Origin::Physical
    };
    match decide::mouse_message(message, m.mouseData) {
        MouseMessage::Move => {
            if m.dwExtraInfo != TAG as usize {
                LAST_MOVE_INJECTED.store(injected, Ordering::Relaxed);
            }
        }
        MouseMessage::Event(e) => report(e, origin),
        MouseMessage::Other => {}
    }
    decide::withhold(mode(), Kind::Mouse, origin)
}

/// Reports the relative movement in a `WM_INPUT` message (design D4).
fn on_raw_input(lparam: LPARAM) {
    let mut raw = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    // SAFETY: `raw` is a writable RAWINPUT of `size` bytes; the handle comes
    // from the WM_INPUT message being handled.
    let n = unsafe {
        GetRawInputData(
            HRAWINPUT(lparam.0 as _),
            RID_INPUT,
            Some((&raw mut raw).cast()),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if n == u32::MAX || raw.header.dwType != RIM_TYPEMOUSE.0 {
        return;
    }
    // SAFETY: the header says this is mouse input, so `mouse` is the member
    // GetRawInputData wrote.
    let m = unsafe { raw.data.mouse };
    let Some((dx, dy)) = decide::raw_motion(m.usFlags.0, m.lLastX, m.lLastY) else {
        return;
    };
    let origin = decide::motion_origin(
        m.ulExtraInformation,
        !raw.header.hDevice.0.is_null(),
        LAST_MOVE_INJECTED.load(Ordering::Relaxed),
    );
    let mut p = POINT::default();
    // SAFETY: `p` is a valid, writable POINT.
    let _ = unsafe { GetCursorPos(&mut p) };
    report(
        InputEvent::Motion {
            dx,
            dy,
            cursor: Point::new(p.x, p.y),
        },
        origin,
    );
}
