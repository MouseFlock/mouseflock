//! The `mouseflock-clipboard` thread (add-text-clipboard design D5,
//! add-clipboard-images D5): a hidden window listening for clipboard changes,
//! reading a copy once the clipboard has been still for [`SETTLE_MS`], and
//! writing the text and images it is given.
//!
//! A copy is read in this order: privacy markers first, so a private copy's
//! contents are never read; then text; then an image, from the `PNG` format
//! as it is, or else from `CF_DIBV5` or `CF_DIB` encoded to PNG; then files,
//! from `CF_HDROP`, walked into a [`FileCopy`] once the clipboard is closed
//! again (add-clipboard-files D1, D2). A received
//! image is decoded on this thread, in order with the text it is given, and
//! set as `PNG` plus `CF_DIBV5`.
//!
//! Its own writes are recognised by the clipboard's sequence number and
//! never reported; files it offers carry a marker format of their own, since
//! another thread places them ([`crate::files`]).

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Once};
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
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer, MSG,
    PostThreadMessageW, RegisterClassW, SetTimer, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WM_CLIPBOARDUPDATE, WM_QUIT, WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW,
};
use windows::core::{PCWSTR, w};

use crate::files::{Entry, FileCopy, LIMITS, Offerer, Source, WalkError, own_format, walk};
use crate::image::{ImageError, check_png, dib_to_png, png_end, png_to_dibv5};
use crate::text::{MAX_TEXT_LEN, Markers, to_utf8, to_utf16z};

/// How long the clipboard must be still before a copy is read.
pub const SETTLE_MS: u32 = 100;
/// How long to keep trying to open a clipboard another app holds.
const OPEN_FOR: Duration = Duration::from_secs(1);
const OPEN_RETRY: Duration = Duration::from_millis(20);
/// The settle timer's id.
const SETTLE: usize = 1;
/// Posted to the thread when a copy is waiting to be set.
const WM_SET: u32 = WM_APP;
const CLASS: PCWSTR = w!("MouseFlockClipboard");

/// What the clipboard held after a copy settled, or why a received image
/// could not be set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Copied {
    Text(String),
    /// An image with no text alongside it, as one PNG within the image
    /// limits: the copying app's own PNG, or its bitmap encoded.
    Image(Vec<u8>),
    /// Files with no text or image alongside them, within the file copy
    /// limits.
    Files(FileCopy),
    /// Marked to stay out of clipboard monitoring, history or cloud sync.
    Private,
    /// Neither text, an image nor files this thread can share.
    NoText,
    /// Text over the 4 MiB limit, an image over the image limits, or files
    /// over the file copy limits.
    TooLarge,
    /// A received image could not be decoded, so the clipboard was left as
    /// it was. Not a copy: nothing is shared for it.
    NotSet(ImageError),
}

/// A copy waiting to be set by the clipboard thread.
enum Set {
    Text(String),
    /// One PNG, decoded on the clipboard thread.
    Image(Vec<u8>),
}

/// The running clipboard thread. Dropping it stops the thread and removes
/// the listener.
pub struct Clipboard {
    thread_id: u32,
    sets: Sender<Set>,
    handle: Option<JoinHandle<()>>,
    /// The thread that offers received files; `None` if OLE would not start.
    files: Option<Offerer>,
}

impl Clipboard {
    /// Replaces the clipboard with `text` as Unicode text, on the clipboard
    /// thread. The change is not reported as a copy.
    pub fn set(&self, text: String) {
        self.queue(Set::Text(text));
    }

    /// Decodes `png` on the clipboard thread and replaces the clipboard with
    /// the image, as `PNG` and `CF_DIBV5`. The change is not reported as a
    /// copy. If it cannot be decoded, or is over the image limits, the
    /// clipboard is left as it was and [`Copied::NotSet`] is reported.
    pub fn set_image(&self, png: Vec<u8>) {
        self.queue(Set::Image(png));
    }

    /// Offers `entries` for paste, without their bytes: a file manager that
    /// pastes reads each file from `source` (add-clipboard-files D3). The
    /// change is not reported as a copy. `false` if files cannot be offered
    /// on this desktop.
    pub fn set_files(&self, entries: Vec<Entry>, source: Arc<dyn Source>) -> bool {
        match &self.files {
            Some(files) => {
                files.offer(entries, source);
                true
            }
            None => false,
        }
    }

    fn queue(&self, set: Set) {
        if self.sets.send(set).is_ok() {
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
        .name("mouseflock-clipboard".into())
        .spawn(move || run(&tx, &set_rx, &ready_tx))
        .map_err(windows::core::Error::from)?;
    match ready_rx.recv() {
        Ok(Ok(thread_id)) => Ok((
            Clipboard {
                thread_id,
                sets,
                handle: Some(handle),
                files: Offerer::start().ok(),
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
    sets: &Receiver<Set>,
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
            while let Ok(set) = sets.try_recv() {
                let written = match set {
                    Set::Text(text) => write(Some(hwnd), &to_utf16z(&text), &[]),
                    Set::Image(png) => match png_to_dibv5(&png) {
                        Ok(dib) => write(
                            Some(hwnd),
                            &[],
                            &[(png_format(), png), (u32::from(CF_DIBV5.0), dib)],
                        ),
                        Err(e) => {
                            let _ = tx.send(Copied::NotSet(e));
                            false
                        }
                    },
                };
                if written {
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
            w!("MouseFlock"),
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

/// The registered `PNG` format, as browsers and the Snipping Tool use it.
pub(crate) fn png_format() -> u32 {
    format(w!("PNG"))
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
/// `None` if the clipboard stayed held by another app, or holds files this
/// agent offered.
pub(crate) fn read(owner: Option<HWND>) -> Option<Copied> {
    let open = open(owner)?;
    if available(&open, own_format()) {
        return None;
    }
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
        if let Some(image) = read_image(&open) {
            return Some(image);
        }
        let paths = read_paths(&open);
        // Walking a tree reads the disk: let other apps have the clipboard.
        drop(open);
        return Some(match paths.map(|p| walk(&p, LIMITS)) {
            Some(Ok(copy)) => Copied::Files(copy),
            Some(Err(WalkError::TooLarge)) => Copied::TooLarge,
            Some(Err(WalkError::Empty | WalkError::Unreadable)) | None => Copied::NoText,
        });
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

/// Where an image may be read from, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageSource {
    /// The app's own PNG, used as it is.
    Png,
    /// A bitmap, encoded to PNG.
    Dib(u32),
}

/// The image sources to try, best first, given which formats are on the
/// clipboard. Only consulted when there is no text: text wins.
pub(crate) fn image_sources(has: impl Fn(u32) -> bool, png: u32) -> Vec<ImageSource> {
    let mut sources = Vec::new();
    if has(png) {
        sources.push(ImageSource::Png);
    }
    for dib in [u32::from(CF_DIBV5.0), u32::from(CF_DIB.0)] {
        if has(dib) {
            sources.push(ImageSource::Dib(dib));
        }
    }
    sources
}

/// The image on the open clipboard, as one PNG, if there is one this thread
/// can share. A source that turns out malformed or unsupported gives way to
/// the next; one over the limits makes the copy too large.
fn read_image(open: &Open) -> Option<Copied> {
    let png = png_format();
    for source in image_sources(|f| available(open, f), png) {
        let got = match source {
            ImageSource::Png => with_data(open, png, |bytes| {
                let bytes = bytes.get(..png_end(bytes)?)?;
                Some(check_png(bytes).map(|_| bytes.to_vec()))
            })
            .flatten(),
            ImageSource::Dib(format) => with_data(open, format, dib_to_png),
        };
        match got {
            Some(Ok(png)) => return Some(Copied::Image(png)),
            Some(Err(ImageError::TooLarge)) => return Some(Copied::TooLarge),
            Some(Err(_)) | None => {}
        }
    }
    None
}

/// The paths of the files on the open clipboard, if it holds `CF_HDROP`.
fn read_paths(open: &Open) -> Option<Vec<PathBuf>> {
    let format = u32::from(CF_HDROP.0);
    if !available(open, format) {
        return None;
    }
    // SAFETY: the clipboard is open on this thread, so the handle stays valid
    // while it is read; DragQueryFileW only reads it, into buffers of the
    // length it reported, and never takes ownership.
    unsafe {
        let hdrop = HDROP(GetClipboardData(format).ok()?.0);
        let count = DragQueryFileW(hdrop, u32::MAX, None);
        let mut paths = Vec::new();
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let got = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
            paths.push(PathBuf::from(OsString::from_wide(buf.get(..got)?)));
        }
        Some(paths)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: u32 = 0xc123;
    const DIBV5: u32 = 17;
    const DIB: u32 = 8;

    fn sources(present: &[u32]) -> Vec<ImageSource> {
        image_sources(|f| present.contains(&f), PNG)
    }

    #[test]
    fn png_first_then_dibv5_then_dib() {
        assert_eq!(u32::from(CF_DIBV5.0), DIBV5);
        assert_eq!(u32::from(CF_DIB.0), DIB);
        assert_eq!(
            sources(&[DIB, PNG, DIBV5]),
            [
                ImageSource::Png,
                ImageSource::Dib(DIBV5),
                ImageSource::Dib(DIB)
            ]
        );
        assert_eq!(sources(&[DIB]), [ImageSource::Dib(DIB)]);
        assert_eq!(sources(&[PNG]), [ImageSource::Png]);
        assert_eq!(sources(&[0xc999]), []);
    }
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
        clipboard.set("mouseflock set test".into());
        quiet(&rx, Duration::from_millis(600));
        assert_eq!(read(None), Some(Copied::Text("mouseflock set test".into())));
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn copy_is_reported_once() {
        let (_clipboard, rx) = start().unwrap();
        for i in 0..3 {
            copy(&format!("mouseflock burst {i}"), &[]);
            std::thread::sleep(Duration::from_millis(15));
        }
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap(),
            Copied::Text("mouseflock burst 2".into())
        );
        quiet(&rx, Duration::from_millis(400));
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn private_copy_is_reported_private() {
        let (_clipboard, rx) = start().unwrap();
        let (exclude, history, cloud) = marker_formats();
        let zero = 0u32.to_le_bytes().to_vec();
        copy("mouseflock secret", &[(cloud, zero.clone())]);
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Copied::Private);
        copy("mouseflock secret", &[(history, zero)]);
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Copied::Private);
        copy("mouseflock secret", &[(exclude, Vec::new())]);
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Copied::Private);
        copy("mouseflock shared", &[(cloud, 1u32.to_le_bytes().to_vec())]);
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap(),
            Copied::Text("mouseflock shared".into())
        );
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn copy_without_text_is_no_text() {
        let (_clipboard, rx) = start().unwrap();
        let custom = format(w!("MouseFlock test format"));
        assert!(write(None, &[], &[(custom, b"not text".to_vec())]));
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Copied::NoText);
    }

    /// A 2 × 2 bottom-up 32 bpp BI_RGB bitmap with these BGRA pixels, top
    /// row first, as an app copying \`CF_DIB\` would make it.
    fn dib_2x2(px: [[u8; 4]; 4]) -> Vec<u8> {
        let mut out = vec![0u8; 40];
        out[0..4].copy_from_slice(&40u32.to_le_bytes());
        out[4..8].copy_from_slice(&2i32.to_le_bytes());
        out[8..12].copy_from_slice(&2i32.to_le_bytes());
        out[12..14].copy_from_slice(&1u16.to_le_bytes());
        out[14..16].copy_from_slice(&32u16.to_le_bytes());
        // Bottom-up: the second row first.
        for p in [px[2], px[3], px[0], px[1]] {
            out.extend_from_slice(&p);
        }
        out
    }

    /// Decodes a PNG to (width, height, RGBA) through the crate's own path.
    fn pixels(png: &[u8]) -> Vec<u8> {
        let dib = png_to_dibv5(png).unwrap();
        dib[124..].to_vec()
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn bitmap_copy_is_reported_as_png() {
        let (_clipboard, rx) = start().unwrap();
        let px = [
            [0, 0, 255, 255],
            [0, 255, 0, 255],
            [255, 0, 0, 255],
            [9, 9, 9, 255],
        ];
        assert!(write(None, &[], &[(u32::from(CF_DIB.0), dib_2x2(px))]));
        let Copied::Image(png) = rx.recv_timeout(WAIT).unwrap() else {
            panic!("not reported as an image");
        };
        assert_eq!(check_png(&png), Ok((2, 2)));
        // The same pixels, bottom-up BGRA in the decoded bitmap.
        assert_eq!(pixels(&png), dib_2x2(px)[40..]);
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn png_copy_is_reported_byte_for_byte() {
        let (_clipboard, rx) = start().unwrap();
        let png = dib_to_png(&dib_2x2([[1, 2, 3, 255]; 4])).unwrap();
        assert!(write(None, &[], &[(png_format(), png.clone())]));
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Copied::Image(png));
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn text_wins_over_an_image() {
        let (_clipboard, rx) = start().unwrap();
        let png = dib_to_png(&dib_2x2([[1, 2, 3, 255]; 4])).unwrap();
        copy("mouseflock cells", &[(png_format(), png)]);
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap(),
            Copied::Text("mouseflock cells".into())
        );
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn private_image_is_reported_private() {
        let (_clipboard, rx) = start().unwrap();
        let (exclude, ..) = marker_formats();
        let png = dib_to_png(&dib_2x2([[1, 2, 3, 255]; 4])).unwrap();
        assert!(write(
            None,
            &[],
            &[(png_format(), png), (exclude, Vec::new())]
        ));
        assert_eq!(rx.recv_timeout(WAIT).unwrap(), Copied::Private);
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn set_image_is_readable_as_a_bitmap_and_not_reported() {
        let (clipboard, rx) = start().unwrap();
        let px = [
            [10, 20, 30, 255],
            [40, 50, 60, 255],
            [70, 80, 90, 255],
            [100, 110, 120, 255],
        ];
        let png = dib_to_png(&dib_2x2(px)).unwrap();
        clipboard.set_image(png.clone());
        quiet(&rx, Duration::from_millis(600));
        // Read back as another app would: the PNG as given, and CF_DIB,
        // which Windows synthesizes from our CF_DIBV5.
        let open = open(None).unwrap();
        let back = with_data(&open, png_format(), <[u8]>::to_vec).unwrap();
        assert_eq!(back.get(..png.len()), Some(&png[..]));
        let dib = with_data(&open, u32::from(CF_DIB.0), <[u8]>::to_vec).unwrap();
        drop(open);
        assert_eq!(
            pixels(&dib_to_png(&dib).unwrap()),
            pixels(&png),
            "CF_DIB holds other pixels"
        );
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn corrupt_image_leaves_the_clipboard_alone() {
        let (clipboard, rx) = start().unwrap();
        copy("mouseflock before", &[]);
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap(),
            Copied::Text("mouseflock before".into())
        );
        let mut bad = dib_to_png(&dib_2x2([[1, 2, 3, 255]; 4])).unwrap();
        let n = bad.len();
        bad[n / 2] ^= 0xff;
        clipboard.set_image(bad);
        assert!(matches!(rx.recv_timeout(WAIT).unwrap(), Copied::NotSet(_)));
        assert_eq!(read(None), Some(Copied::Text("mouseflock before".into())));
    }

    /// `CF_HDROP` bytes for `paths`, as Explorer puts them on the clipboard:
    /// a `DROPFILES` header, then each path NUL-terminated, then a NUL.
    fn hdrop(paths: &[PathBuf]) -> Vec<u8> {
        use std::os::windows::ffi::OsStrExt;
        let mut out = Vec::new();
        out.extend_from_slice(&20u32.to_le_bytes()); // pFiles
        out.extend_from_slice(&[0; 8]); // pt
        out.extend_from_slice(&0u32.to_le_bytes()); // fNC
        out.extend_from_slice(&1u32.to_le_bytes()); // fWide
        for p in paths {
            for u in p.as_os_str().encode_wide().chain([0]) {
                out.extend_from_slice(&u.to_le_bytes());
            }
        }
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn copied_files_are_reported_as_a_file_copy() {
        let (_clipboard, rx) = start().unwrap();
        let dir = std::env::temp_dir().join("mouseflock-hdrop");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("folder").join("empty")).unwrap();
        std::fs::write(dir.join("folder").join("a.txt"), b"abc").unwrap();
        std::fs::write(dir.join("b.txt"), b"hello").unwrap();
        let paths = [dir.join("folder"), dir.join("b.txt")];
        assert!(write(None, &[], &[(u32::from(CF_HDROP.0), hdrop(&paths))]));
        let Copied::Files(copy) = rx.recv_timeout(WAIT).unwrap() else {
            panic!("not reported as files");
        };
        let listed: Vec<_> = copy
            .entries
            .iter()
            .map(|e| (e.path.as_str(), e.is_dir, e.size))
            .collect();
        assert_eq!(
            listed,
            [
                ("folder", true, 0),
                ("folder/a.txt", false, 3),
                ("folder/empty", true, 0),
                ("b.txt", false, 5),
            ]
        );
        assert_eq!(copy.sources[3], dir.join("b.txt"));
    }

    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn text_wins_over_files() {
        let (_clipboard, rx) = start().unwrap();
        let file = std::env::temp_dir().join("mouseflock-hdrop-text.txt");
        std::fs::write(&file, b"x").unwrap();
        copy(
            "mouseflock with files",
            &[(u32::from(CF_HDROP.0), hdrop(&[file]))],
        );
        assert_eq!(
            rx.recv_timeout(WAIT).unwrap(),
            Copied::Text("mouseflock with files".into())
        );
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
