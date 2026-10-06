//! The `deskhop-clipboard` thread (add-text-clipboard design D5): a hidden
//! window listening for clipboard changes, reading a copy once the clipboard
//! has been still for [`SETTLE_MS`], and writing text it is given.
//!
//! Its own writes are recognised by the clipboard's sequence number and
//! never reported.

use std::sync::Once;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, RemoveClipboardFormatListener, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer, MSG,
    PostThreadMessageW, RegisterClassW, SetTimer, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WM_CLIPBOARDUPDATE, WM_QUIT, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW,
};
use windows::core::{PCWSTR, w};

use crate::text::{MAX_TEXT_LEN, Markers, to_utf8, to_utf16z};

/// How long the clipboard must be still before a copy is read.
pub const SETTLE_MS: u32 = 100;
/// How long to keep trying to open a clipboard another app holds.
const OPEN_FOR: Duration = Duration::from_secs(1);
const OPEN_RETRY: Duration = Duration::from_millis(20);
/// The settle timer's id.
const SETTLE: usize = 1;
/// Posted to the thread when text is waiting to be set.
const WM_SET: u32 = WM_APP;
const CLASS: PCWSTR = w!("DeskHopClipboard");

/// What the clipboard held after a copy settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Copied {
    Text(String),
    /// Marked to stay out of clipboard monitoring, history or cloud sync.
    Private,
    /// No Unicode text on the clipboard.
    NoText,
    /// Text over the 4 MiB limit.
    TooLarge,
}

/// The running clipboard thread. Dropping it stops the thread and removes
/// the listener.
pub struct Clipboard {
    thread_id: u32,
    sets: Sender<String>,
    handle: Option<JoinHandle<()>>,
}

impl Clipboard {
    /// Replaces the clipboard with `text` as Unicode text, on the clipboard
    /// thread. The change is not reported as a copy.
    pub fn set(&self, text: String) {
        if self.sets.send(text).is_ok() {
            // SAFETY: posts a message with no pointers to a thread whose
            // queue exists (it was created before the thread reported ready).
            let _ = unsafe { PostThreadMessageW(self.thread_id, WM_SET, WPARAM(0), LPARAM(0)) };
        }
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        // SAFETY: as in `set`.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Starts the clipboard thread. Copies arrive on the receiver.
pub fn start() -> windows::core::Result<(Clipboard, Receiver<Copied>)> {
    let (tx, rx) = mpsc::channel();
    let (sets, set_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let handle = std::thread::Builder::new()
        .name("deskhop-clipboard".into())
        .spawn(move || run(&tx, &set_rx, &ready_tx))
        .map_err(windows::core::Error::from)?;
    match ready_rx.recv() {
        Ok(Ok(thread_id)) => Ok((
            Clipboard {
                thread_id,
                sets,
                handle: Some(handle),
            },
            rx,
        )),
        Ok(Err(e)) => {
            let _ = handle.join();
            Err(e)
        }
        Err(_) => {
            let _ = handle.join();
            Err(windows::core::Error::from_win32())
        }
    }
}

fn run(
    tx: &Sender<Copied>,
    sets: &Receiver<String>,
    ready: &mpsc::SyncSender<windows::core::Result<u32>>,
) {
    let hwnd = match window() {
        Ok(h) => h,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    // SAFETY: the window belongs to this thread.
    if let Err(e) = unsafe { AddClipboardFormatListener(hwnd) } {
        let _ = ready.send(Err(e));
        // SAFETY: the window was created on this thread.
        let _ = unsafe { DestroyWindow(hwnd) };
        return;
    }
    // SAFETY: no arguments; cannot fail.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));

    // The sequence number right after this thread's last write.
    let mut own = None;
    message_loop(|msg| {
        if msg.message == WM_TIMER && msg.hwnd == hwnd && msg.wParam.0 == SETTLE {
            // SAFETY: the timer belongs to this thread's window.
            let _ = unsafe { KillTimer(Some(hwnd), SETTLE) };
            // SAFETY: no arguments.
            if own == Some(unsafe { GetClipboardSequenceNumber() }) {
                return;
            }
            if let Some(copied) = read(Some(hwnd)) {
                let _ = tx.send(copied);
            }
        } else if msg.message == WM_SET {
            while let Ok(text) = sets.try_recv() {
                if write(Some(hwnd), &to_utf16z(&text), &[]) {
                    // SAFETY: no arguments.
                    own = Some(unsafe { GetClipboardSequenceNumber() });
                }
            }
        }
    });
    // SAFETY: the window was created on this thread and is still listening.
    unsafe {
        let _ = RemoveClipboardFormatListener(hwnd);
        let _ = DestroyWindow(hwnd);
    }
}

/// A hidden top-level window of our own class: `WM_CLIPBOARDUPDATE` is sent
/// to the window procedure, which the message loop never sees.
fn window() -> windows::core::Result<HWND> {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        // SAFETY: the class name is static and the procedure has the WNDPROC
        // signature. A failure shows up as CreateWindowExW failing.
        unsafe {
            if let Ok(instance) = GetModuleHandleW(None) {
                let class = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: instance.into(),
                    lpszClassName: CLASS,
                    ..Default::default()
                };
                RegisterClassW(&class);
            }
        }
    });
    // SAFETY: the class was registered above; all other arguments are plain
    // values or absent.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WINDOW_EX_STYLE(0),
            CLASS,
            w!("DeskHop"),
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

/// (Re)arms the settle timer on every clipboard change, so a burst of
/// changes is read once.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        // SAFETY: the window is this thread's; no callback is used. Setting
        // an existing timer id restarts it.
        unsafe { SetTimer(Some(hwnd), SETTLE, SETTLE_MS, None) };
        return LRESULT(0);
    }
    // SAFETY: forwards the arguments Windows passed in, unchanged.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Runs a message loop until `WM_QUIT`, handing each message to `on`
/// before dispatching it.
fn message_loop(mut on: impl FnMut(&MSG)) {
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

// ---- the clipboard itself ------------------------------------------------------

/// The clipboard, open; closed on drop.
struct Open;

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: this thread opened the clipboard.
        let _ = unsafe { CloseClipboard() };
    }
}

/// Opens the clipboard for `owner`, retrying while another app holds it.
fn open(owner: Option<HWND>) -> Option<Open> {
    let deadline = Instant::now() + OPEN_FOR;
    loop {
        // SAFETY: `owner` is a window of this thread, or none.
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return Some(Open);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(OPEN_RETRY);
    }
}

/// A registered format's id by name.
fn format(name: PCWSTR) -> u32 {
    // SAFETY: `name` is a static NUL-terminated string.
    unsafe { RegisterClipboardFormatW(name) }
}

/// The privacy markers' format ids.
pub(crate) fn marker_formats() -> (u32, u32, u32) {
    (
        format(w!("ExcludeClipboardContentFromMonitorProcessing")),
        format(w!("CanIncludeInClipboardHistory")),
        format(w!("CanUploadToCloudClipboard")),
    )
}

/// Whether `format` is on the open clipboard.
fn available(_open: &Open, format: u32) -> bool {
    // SAFETY: plain value.
    unsafe { IsClipboardFormatAvailable(format) }.is_ok()
}

/// Runs `f` on the bytes of `format`'s data on the open clipboard.
fn with_data<T>(_open: &Open, format: u32, f: impl FnOnce(&[u8]) -> T) -> Option<T> {
    // SAFETY: the clipboard is open on this thread. The handle stays valid
    // while it is open; it is locked while its bytes are read, and the
    // slice does not outlive the lock.
    unsafe {
        let handle = GetClipboardData(format).ok()?;
        let global = HGLOBAL(handle.0);
        let size = GlobalSize(global);
        let p = GlobalLock(global).cast::<u8>();
        if p.is_null() {
            return None;
        }
        let out = f(std::slice::from_raw_parts(p, size));
        let _ = GlobalUnlock(global);
        Some(out)
    }
}

/// A DWORD marker's value, if present.
fn dword(open: &Open, format: u32) -> Option<u32> {
    if !available(open, format) {
        return None;
    }
    with_data(open, format, |b| {
        b.first_chunk::<4>().map(|d| u32::from_le_bytes(*d))
    })
    .flatten()
}

/// Reads the clipboard, privacy first: a private copy's text is never read.
/// `None` if the clipboard stayed held by another app.
pub(crate) fn read(owner: Option<HWND>) -> Option<Copied> {
    let open = open(owner)?;
    let (exclude, history, cloud) = marker_formats();
    let markers = Markers {
        exclude: available(&open, exclude),
        history: dword(&open, history),
        cloud: dword(&open, cloud),
    };
    if markers.private() {
        return Some(Copied::Private);
    }
    let text = u32::from(CF_UNICODETEXT.0);
    if !available(&open, text) {
        return Some(Copied::NoText);
    }
    let read = with_data(&open, text, |bytes| {
        // Each UTF-16 unit is at least one byte of UTF-8: reject the
        // obviously oversized before converting.
        if bytes.len() / 2 > MAX_TEXT_LEN + 1 {
            return Copied::TooLarge;
        }
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u16::from_le_bytes(*p))
            .collect();
        match to_utf8(&units) {
            Ok(t) => Copied::Text(t),
            Err(_) => Copied::TooLarge,
        }
    });
    Some(read.unwrap_or(Copied::NoText))
}

/// Puts `data` for `format` on the open, emptied clipboard.
fn put(_open: &Open, format: u32, data: &[u8]) -> bool {
    // SAFETY: the memory is allocated, filled while locked, and handed to
    // the clipboard, which owns it on success; on failure it is freed here.
    unsafe {
        let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, data.len().max(1)) else {
            return false;
        };
        let p = GlobalLock(global).cast::<u8>();
        if p.is_null() {
            let _ = GlobalFree(Some(global));
            return false;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        let _ = GlobalUnlock(global);
        if SetClipboardData(format, Some(HANDLE(global.0))).is_err() {
            let _ = GlobalFree(Some(global));
            return false;
        }
        true
    }
}

/// Replaces the clipboard with `units` (NUL-terminated UTF-16) as Unicode
/// text, plus `extra` formats, all owned by `owner`.
pub(crate) fn write(owner: Option<HWND>, units: &[u16], extra: &[(u32, Vec<u8>)]) -> bool {
    let Some(open) = open(owner) else {
        return false;
    };
    // SAFETY: the clipboard is open on this thread.
    if unsafe { EmptyClipboard() }.is_err() {
        return false;
    }
    let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
    if !units.is_empty() && !put(&open, u32::from(CF_UNICODETEXT.0), &bytes) {
        return false;
    }
    extra.iter().all(|(format, data)| put(&open, *format, data))
}

/// Against the real clipboard, so they need a logged-on desktop, replace
/// whatever was copied, and must not run in parallel:
///
/// ```text
/// cargo test -p win32-clipboard -- --ignored --test-threads=1
/// ```
#[cfg(test)]
mod desktop {
    use super::*;

    const WAIT: Duration = Duration::from_secs(2);

    /// Copies as another app would: not through the listener's thread, so
    /// its sequence number is not recorded as the listener's own.
    fn copy(text: &str, extra: &[(u32, Vec<u8>)]) {
        assert!(write(None, &to_utf16z(text), extra), "could not copy");
    }

    fn quiet(rx: &Receiver<Copied>, for_: Duration) {
        if let Ok(c) = rx.recv_timeout(for_) {
            panic!("unexpected report {c:?}");
        }
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn set_is_not_reported() {
        let (clipboard, rx) = start().unwrap();
        clipboard.set("deskhop set test".into());
        quiet(&rx, Duration::from_millis(600));
        assert_eq!(read(None), Some(Copied::Text("deskhop set test".into())));
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn copy_is_reported_once() {
        let (_clipboard, rx) = start().unwrap();
        for i in 0..3 {
            copy(&format!("deskhop burst {i}"), &[]);
            std::thread::sleep(Duration::from_millis(15));
        }
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap(),
            Copied::Text("deskhop burst 2".into())
        );
        quiet(&rx, Duration::from_millis(400));
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn private_copy_is_reported_private() {
        let (_clipboard, rx) = start().unwrap();
        let (exclude, history, cloud) = marker_formats();
        let zero = 0u32.to_le_bytes().to_vec();
        copy("deskhop secret", &[(cloud, zero.clone())]);
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Copied::Private);
        copy("deskhop secret", &[(history, zero)]);
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Copied::Private);
        copy("deskhop secret", &[(exclude, Vec::new())]);
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Copied::Private);
        copy("deskhop shared", &[(cloud, 1u32.to_le_bytes().to_vec())]);
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap(),
            Copied::Text("deskhop shared".into())
        );
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn copy_without_text_is_no_text() {
        let (_clipboard, rx) = start().unwrap();
        let custom = format(w!("DeskHop test format"));
        assert!(write(None, &[], &[(custom, b"not text".to_vec())]));
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Copied::NoText);
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn text_round_trips() {
        let (clipboard, _rx) = start().unwrap();
        let text = "héllo 😀\r\nsecond line\ttab";
        clipboard.set(text.into());
        let deadline = Instant::now() + WAIT;
        while read(None) != Some(Copied::Text(text.into())) {
            assert!(Instant::now() < deadline, "text never arrived");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
