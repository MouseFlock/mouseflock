//! Files offered for paste without their bytes (add-clipboard-files D3).
//!
//! [`data_object`] makes an OLE data object offering `FileGroupDescriptorW`
//! (every entry's relative name, directory flag and size) and
//! `FileContents` as one `IStream` per file. A file manager pasting it
//! creates the tree from the descriptor, then reads each file's stream,
//! and only then does the [`Source`] open that file. Nothing is read before
//! the paste.
//!
//! The descriptor is built by a pure function, [`descriptor`]; the COM
//! objects only hand out what it built and what the source reads.
//!
//! The other way, [`walk`] turns the paths of a copy made in Explorer into a
//! [`FileCopy`]: every file and directory under them, with the path on disk
//! each came from, so its bytes can be served when another machine pastes.

use std::ffi::c_void;
use std::fs;
use std::io::{self, Read};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    DATA_S_SAMEFORMATETC, DV_E_DVASPECT, DV_E_FORMATETC, DV_E_LINDEX, DV_E_TYMED, E_NOTIMPL,
    E_OUTOFMEMORY, E_POINTER, GlobalFree, HGLOBAL, LPARAM, OLE_E_ADVISENOTSUPPORTED, S_FALSE, S_OK,
    STG_E_ACCESSDENIED, STG_E_INVALIDFUNCTION, STG_E_INVALIDPOINTER, STG_E_READFAULT, WPARAM,
};
use windows::Win32::System::Com::{
    DATADIR_GET, DVASPECT_CONTENT, FORMATETC, IAdviseSink, IDataObject, IDataObject_Impl,
    IEnumFORMATETC, IEnumSTATDATA, ISequentialStream_Impl, IStream, IStream_Impl, LOCKTYPE,
    STATFLAG, STATSTG, STGC, STGM_READ, STGMEDIUM, STGMEDIUM_0, STGTY_STREAM, STREAM_SEEK,
    STREAM_SEEK_CUR, STREAM_SEEK_SET, TYMED_HGLOBAL, TYMED_ISTREAM,
};
use windows::Win32::System::DataExchange::{IsClipboardFormatAvailable, RegisterClipboardFormatW};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{
    DROPEFFECT_COPY, OleInitialize, OleSetClipboard, OleUninitialize, ReleaseStgMedium,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{
    CFSTR_FILECONTENTS, CFSTR_FILEDESCRIPTORW, CFSTR_PREFERREDDROPEFFECT, FD_ATTRIBUTES,
    FD_FILESIZE, FD_PROGRESSUI, FD_UNICODE, SHCreateStdEnumFmtEtc,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PostThreadMessageW, WM_APP, WM_QUIT,
};
use windows_core::{BOOL, HRESULT, Ref, implement, w};

/// One entry of a file copy: a file or a directory, by its path relative to
/// the copy root with `/` separators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub is_dir: bool,
    /// The file's length in bytes; 0 for a directory.
    pub size: u64,
}

/// Why a list of entries cannot be offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorError {
    /// Nothing to offer.
    Empty,
    /// The entry at this index has a path longer than a descriptor holds:
    /// 259 UTF-16 units.
    PathTooLong(usize),
}

/// Opens a file of the offered copy, by its index in the entries, when the
/// file manager reads it on paste.
pub trait Source: Send + Sync {
    fn open(&self, index: usize) -> io::Result<Box<dyn Read + Send>>;
}

// `FILEDESCRIPTORW`'s layout (packed, 592 bytes); checked against the
// Windows type in the tests.
const FD_SIZE: usize = 592;
const FD_FLAGS_AT: usize = 0;
const FD_ATTRIBUTES_AT: usize = 36;
const FD_SIZE_HIGH_AT: usize = 64;
const FD_SIZE_LOW_AT: usize = 68;
const FD_NAME_AT: usize = 72;
/// `cFileName`'s length in UTF-16 units, with the terminating NUL.
const FD_NAME_UNITS: usize = 260;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

/// The `FileGroupDescriptorW` bytes for `entries`: a count, then one
/// `FILEDESCRIPTORW` each, with the path (`\` separated), the directory
/// attribute and the size, and the progress UI asked for.
pub fn descriptor(entries: &[Entry]) -> Result<Vec<u8>, DescriptorError> {
    if entries.is_empty() {
        return Err(DescriptorError::Empty);
    }
    let count = u32::try_from(entries.len()).map_err(|_| DescriptorError::Empty)?;
    let mut out = Vec::with_capacity(4 + entries.len() * FD_SIZE);
    out.extend_from_slice(&count.to_le_bytes());
    let flags = (FD_ATTRIBUTES.0 | FD_FILESIZE.0 | FD_PROGRESSUI.0 | FD_UNICODE.0).cast_unsigned();
    for (i, e) in entries.iter().enumerate() {
        let name: Vec<u16> = e.path.replace('/', "\\").encode_utf16().collect();
        if name.len() >= FD_NAME_UNITS {
            return Err(DescriptorError::PathTooLong(i));
        }
        let mut fd = [0u8; FD_SIZE];
        fd[FD_FLAGS_AT..][..4].copy_from_slice(&flags.to_le_bytes());
        let attributes = if e.is_dir {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            FILE_ATTRIBUTE_NORMAL
        };
        fd[FD_ATTRIBUTES_AT..][..4].copy_from_slice(&attributes.to_le_bytes());
        let size = if e.is_dir { 0 } else { e.size };
        fd[FD_SIZE_HIGH_AT..][..4].copy_from_slice(&((size >> 32) as u32).to_le_bytes());
        fd[FD_SIZE_LOW_AT..][..4].copy_from_slice(&(size as u32).to_le_bytes());
        for (j, unit) in name.iter().enumerate() {
            fd[FD_NAME_AT + 2 * j..][..2].copy_from_slice(&unit.to_le_bytes());
        }
        out.extend_from_slice(&fd);
    }
    Ok(out)
}

/// A file copy read from the clipboard: its entries, in manifest order, and
/// where on disk each one is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileCopy {
    pub entries: Vec<Entry>,
    /// The path of each entry, by index.
    pub sources: Vec<PathBuf>,
}

/// The limits a walk keeps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Most entries, files and directories.
    pub entries: usize,
    /// Most bytes the files may total.
    pub total: u64,
    /// Longest relative path, in UTF-16 code units.
    pub path_units: usize,
}

/// The file copy limits (add-clipboard-files D6).
pub const LIMITS: Limits = Limits {
    entries: 65_536,
    total: 68_719_476_736,
    path_units: 259,
};

/// Why a copy's paths are not shared as files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkError {
    /// Nothing to share.
    Empty,
    /// Over a limit: too many entries, too many bytes, or a path too long.
    TooLarge,
    /// A name that is not valid Unicode, or a file or folder that cannot be
    /// read.
    Unreadable,
}

/// Every file and directory under `roots`, each root named by its own name,
/// directories before what they hold and names in order. A link to a file
/// is followed; a link to a directory, or a junction, is left out, so a
/// walk never loops. Stops as soon as a limit is passed.
pub fn walk(roots: &[PathBuf], limits: Limits) -> Result<FileCopy, WalkError> {
    let mut walk = Walk {
        copy: FileCopy::default(),
        total: 0,
        limits,
    };
    for root in roots {
        let name = root
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(WalkError::Unreadable)?;
        walk.add(root, name.to_owned())?;
    }
    if walk.copy.entries.is_empty() {
        return Err(WalkError::Empty);
    }
    Ok(walk.copy)
}

struct Walk {
    copy: FileCopy,
    total: u64,
    limits: Limits,
}

impl Walk {
    fn add(&mut self, path: &Path, rel: String) -> Result<(), WalkError> {
        if rel.encode_utf16().count() > self.limits.path_units
            || self.copy.entries.len() >= self.limits.entries
        {
            return Err(WalkError::TooLarge);
        }
        let meta = fs::symlink_metadata(path).map_err(|_| WalkError::Unreadable)?;
        if meta.file_type().is_symlink() {
            return match fs::metadata(path) {
                Ok(target) if target.is_file() => self.file(path, rel, target.len()),
                _ => Ok(()),
            };
        }
        if !meta.is_dir() {
            return self.file(path, rel, meta.len());
        }
        self.push(path, rel.clone(), true, 0);
        let mut names = Vec::new();
        for child in fs::read_dir(path).map_err(|_| WalkError::Unreadable)? {
            let child = child.map_err(|_| WalkError::Unreadable)?;
            let name = child
                .file_name()
                .into_string()
                .map_err(|_| WalkError::Unreadable)?;
            names.push(name);
        }
        names.sort();
        for name in names {
            self.add(&path.join(&name), format!("{rel}/{name}"))?;
        }
        Ok(())
    }

    fn file(&mut self, path: &Path, rel: String, size: u64) -> Result<(), WalkError> {
        self.total = self
            .total
            .checked_add(size)
            .filter(|t| *t <= self.limits.total)
            .ok_or(WalkError::TooLarge)?;
        self.push(path, rel, false, size);
        Ok(())
    }

    fn push(&mut self, path: &Path, rel: String, is_dir: bool, size: u64) {
        self.copy.entries.push(Entry {
            path: rel,
            is_dir,
            size,
        });
        self.copy.sources.push(path.to_path_buf());
    }
}

/// An OLE data object offering `entries` for paste. A file's bytes are read
/// from `source` only when the file manager reads that file's stream.
pub fn data_object(
    entries: Vec<Entry>,
    source: Arc<dyn Source>,
) -> Result<IDataObject, DescriptorError> {
    let descriptor = descriptor(&entries)?;
    Ok(Files {
        descriptor,
        entries,
        source,
        formats: Formats::registered(),
    }
    .into())
}

/// The registered format ids the data object offers.
#[derive(Clone, Copy)]
struct Formats {
    descriptor: u16,
    contents: u16,
    effect: u16,
    /// Marks the copy as this agent's own, so the listener never reports it.
    own: u16,
}

impl Formats {
    fn registered() -> Self {
        // SAFETY: each name is a static NUL-terminated string. Registered
        // format ids fit in 16 bits (0xC000 to 0xFFFF).
        let id = |name| unsafe { RegisterClipboardFormatW(name) } as u16;
        Self {
            descriptor: id(CFSTR_FILEDESCRIPTORW),
            contents: id(CFSTR_FILECONTENTS),
            effect: id(CFSTR_PREFERREDDROPEFFECT),
            own: id(w!("DeskHopFiles")),
        }
    }
}

/// The format that marks files this agent offered, so the listener does not
/// report them as a copy.
pub(crate) fn own_format() -> u32 {
    u32::from(Formats::registered().own)
}

/// What a `FORMATETC` asks the data object for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asked {
    Descriptor,
    Contents(usize),
    Effect,
    Own,
}

/// Which offered format `f` names, or the `DV_E_*` error for it.
fn asked(f: &FORMATETC, formats: Formats, entries: &[Entry]) -> Result<Asked, HRESULT> {
    if f.dwAspect != DVASPECT_CONTENT.0 {
        return Err(DV_E_DVASPECT);
    }
    let (asked, tymed) = if f.cfFormat == formats.descriptor {
        (Asked::Descriptor, TYMED_HGLOBAL)
    } else if f.cfFormat == formats.effect {
        (Asked::Effect, TYMED_HGLOBAL)
    } else if f.cfFormat == formats.own {
        (Asked::Own, TYMED_HGLOBAL)
    } else if f.cfFormat == formats.contents {
        let index = usize::try_from(f.lindex).map_err(|_| DV_E_LINDEX)?;
        match entries.get(index) {
            Some(e) if !e.is_dir => (Asked::Contents(index), TYMED_ISTREAM),
            _ => return Err(DV_E_LINDEX),
        }
    } else {
        return Err(DV_E_FORMATETC);
    };
    if f.tymed & tymed.0.cast_unsigned() == 0 {
        return Err(DV_E_TYMED);
    }
    Ok(asked)
}

/// Global memory holding `bytes`, for an `HGLOBAL` medium.
fn global(bytes: &[u8]) -> windows_core::Result<HGLOBAL> {
    // SAFETY: the memory is allocated, filled while locked, and returned to
    // the caller, who hands it to COM; on failure it is freed here.
    unsafe {
        let g = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let p = GlobalLock(g).cast::<u8>();
        if p.is_null() {
            let _ = GlobalFree(Some(g));
            return Err(E_OUTOFMEMORY.into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(g);
        Ok(g)
    }
}

#[implement(IDataObject)]
struct Files {
    descriptor: Vec<u8>,
    entries: Vec<Entry>,
    source: Arc<dyn Source>,
    formats: Formats,
}

impl IDataObject_Impl for Files_Impl {
    fn GetData(&self, f: *const FORMATETC) -> windows_core::Result<STGMEDIUM> {
        // SAFETY: COM passes a valid FORMATETC, or null.
        let f = unsafe { f.as_ref() }.ok_or(E_POINTER)?;
        match asked(f, self.formats, &self.entries)? {
            Asked::Descriptor => Ok(hglobal_medium(global(&self.descriptor)?)),
            Asked::Effect => Ok(hglobal_medium(global(&DROPEFFECT_COPY.0.to_le_bytes())?)),
            Asked::Own => Ok(hglobal_medium(global(&[1])?)),
            Asked::Contents(index) => {
                let reader = self
                    .source
                    .open(index)
                    .map_err(|_| windows_core::Error::from(STG_E_READFAULT))?;
                let stream: IStream = Content {
                    reader: Mutex::new(reader),
                    size: self.entries[index].size,
                    pos: AtomicU64::new(0),
                }
                .into();
                Ok(STGMEDIUM {
                    tymed: TYMED_ISTREAM.0.cast_unsigned(),
                    u: STGMEDIUM_0 {
                        pstm: ManuallyDrop::new(Some(stream)),
                    },
                    pUnkForRelease: ManuallyDrop::new(None),
                })
            }
        }
    }

    fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> windows_core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn QueryGetData(&self, f: *const FORMATETC) -> HRESULT {
        // SAFETY: as in GetData.
        match unsafe { f.as_ref() } {
            None => E_POINTER,
            Some(f) => asked(f, self.formats, &self.entries).map_or_else(|e| e, |_| S_OK),
        }
    }

    fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, out: *mut FORMATETC) -> HRESULT {
        // SAFETY: COM passes a writable FORMATETC, or null.
        if let Some(out) = unsafe { out.as_mut() } {
            out.ptd = std::ptr::null_mut();
        }
        DATA_S_SAMEFORMATETC
    }

    fn SetData(
        &self,
        _: *const FORMATETC,
        medium: *const STGMEDIUM,
        release: BOOL,
    ) -> windows_core::Result<()> {
        // The shell reports how a paste went ("Performed DropEffect",
        // "Paste Succeeded"); nothing here needs it. Owning the medium,
        // release it.
        if release.as_bool() && !medium.is_null() {
            // SAFETY: with `release` set the caller gave us ownership of a
            // valid medium, released exactly once here.
            unsafe { ReleaseStgMedium(medium.cast_mut()) };
        }
        Ok(())
    }

    fn EnumFormatEtc(&self, direction: u32) -> windows_core::Result<IEnumFORMATETC> {
        if direction != DATADIR_GET.0.cast_unsigned() {
            return Err(E_NOTIMPL.into());
        }
        let etc = |cf: u16, tymed: i32| FORMATETC {
            cfFormat: cf,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: tymed.cast_unsigned(),
        };
        let formats = [
            etc(self.formats.descriptor, TYMED_HGLOBAL.0),
            etc(self.formats.contents, TYMED_ISTREAM.0),
            etc(self.formats.effect, TYMED_HGLOBAL.0),
            etc(self.formats.own, TYMED_HGLOBAL.0),
        ];
        // SAFETY: the slice is valid for the call; the enumerator copies it.
        unsafe { SHCreateStdEnumFmtEtc(&formats) }
    }

    fn DAdvise(
        &self,
        _: *const FORMATETC,
        _: u32,
        _: Ref<'_, IAdviseSink>,
    ) -> windows_core::Result<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn DUnadvise(&self, _: u32) -> windows_core::Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn EnumDAdvise(&self) -> windows_core::Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

fn hglobal_medium(g: HGLOBAL) -> STGMEDIUM {
    STGMEDIUM {
        tymed: TYMED_HGLOBAL.0.cast_unsigned(),
        u: STGMEDIUM_0 { hGlobal: g },
        pUnkForRelease: ManuallyDrop::new(None),
    }
}

/// One file's contents, read forward only, as the file manager pulls them.
/// It ends at the file's announced size: a source that ends sooner is a
/// read fault, so a paste fails rather than writing a short file.
#[implement(IStream)]
struct Content {
    reader: Mutex<Box<dyn Read + Send>>,
    size: u64,
    pos: AtomicU64,
}

impl Content {
    /// Fills as much of `buf` as the file has left. `Err` if the source
    /// fails, or ends before the announced size.
    fn fill(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut reader = self
            .reader
            .lock()
            .map_err(|_| io::Error::other("poisoned"))?;
        let pos = self.pos.load(Ordering::Acquire);
        let left = usize::try_from(self.size - pos).unwrap_or(usize::MAX);
        let want = buf.len().min(left);
        let mut n = 0;
        while n < want {
            match reader.read(&mut buf[n..want]) {
                Ok(0) => {
                    self.pos.store(pos + n as u64, Ordering::Release);
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                Ok(k) => n += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    self.pos.store(pos + n as u64, Ordering::Release);
                    return Err(e);
                }
            }
        }
        self.pos.store(pos + n as u64, Ordering::Release);
        Ok(n)
    }
}

impl ISequentialStream_Impl for Content_Impl {
    fn Read(&self, pv: *mut c_void, cb: u32, read: *mut u32) -> HRESULT {
        if pv.is_null() {
            return STG_E_INVALIDPOINTER;
        }
        // SAFETY: COM passes a writable buffer of `cb` bytes.
        let buf = unsafe { std::slice::from_raw_parts_mut(pv.cast::<u8>(), cb as usize) };
        let (n, hr) = match self.fill(buf) {
            Ok(n) if n == buf.len() => (n, S_OK),
            Ok(n) => (n, S_FALSE),
            Err(_) => (0, STG_E_READFAULT),
        };
        // SAFETY: COM passes a writable u32, or null.
        if let Some(read) = unsafe { read.as_mut() } {
            *read = n as u32;
        }
        hr
    }

    fn Write(&self, _: *const c_void, _: u32, _: *mut u32) -> HRESULT {
        STG_E_ACCESSDENIED
    }
}

impl IStream_Impl for Content_Impl {
    /// Forward-only: only a seek that stays put (or asks where it is)
    /// succeeds.
    fn Seek(&self, by: i64, origin: STREAM_SEEK, new: *mut u64) -> windows_core::Result<()> {
        let pos = self.pos.load(Ordering::Acquire);
        let target = match origin {
            STREAM_SEEK_SET => u64::try_from(by).ok(),
            STREAM_SEEK_CUR => pos.checked_add_signed(by),
            _ => None,
        };
        if target != Some(pos) {
            return Err(STG_E_INVALIDFUNCTION.into());
        }
        // SAFETY: COM passes a writable u64, or null.
        if let Some(new) = unsafe { new.as_mut() } {
            *new = pos;
        }
        Ok(())
    }

    fn SetSize(&self, _: u64) -> windows_core::Result<()> {
        Err(STG_E_ACCESSDENIED.into())
    }

    fn CopyTo(
        &self,
        to: Ref<'_, IStream>,
        cb: u64,
        read: *mut u64,
        written: *mut u64,
    ) -> windows_core::Result<()> {
        let to = to.ok()?;
        let mut buf = vec![0u8; 64 * 1024];
        let (mut total_read, mut total_written) = (0u64, 0u64);
        let result = loop {
            let want = usize::try_from(cb - total_read)
                .unwrap_or(usize::MAX)
                .min(buf.len());
            if want == 0 {
                break Ok(());
            }
            let n = match self.fill(&mut buf[..want]) {
                Ok(0) => break Ok(()),
                Ok(n) => n,
                Err(_) => break Err(STG_E_READFAULT.into()),
            };
            total_read += n as u64;
            let mut put = 0u32;
            // SAFETY: the buffer holds `n` bytes for the call; `put` is a
            // writable u32.
            let hr = unsafe { to.Write(buf.as_ptr().cast(), n as u32, Some(&mut put)) };
            total_written += u64::from(put);
            if hr.is_err() {
                break Err(hr.into());
            }
        };
        // SAFETY: COM passes writable u64s, or null.
        unsafe {
            if let Some(read) = read.as_mut() {
                *read = total_read;
            }
            if let Some(written) = written.as_mut() {
                *written = total_written;
            }
        }
        result
    }

    fn Commit(&self, _: &STGC) -> windows_core::Result<()> {
        Ok(())
    }

    fn Revert(&self) -> windows_core::Result<()> {
        Ok(())
    }

    fn LockRegion(&self, _: u64, _: u64, _: &LOCKTYPE) -> windows_core::Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn UnlockRegion(&self, _: u64, _: u64, _: u32) -> windows_core::Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn Stat(&self, stat: *mut STATSTG, _: &STATFLAG) -> windows_core::Result<()> {
        // SAFETY: COM passes a writable STATSTG, or null.
        let stat = unsafe { stat.as_mut() }.ok_or(STG_E_INVALIDPOINTER)?;
        *stat = STATSTG {
            r#type: STGTY_STREAM.0.cast_unsigned(),
            cbSize: self.size,
            grfMode: STGM_READ,
            ..Default::default()
        };
        Ok(())
    }

    fn Clone(&self) -> windows_core::Result<IStream> {
        Err(E_NOTIMPL.into())
    }
}

/// The `deskhop-files` thread (D3): an OLE apartment of its own, pumping
/// messages, that places offered files on the clipboard. A file manager's
/// calls on the data object, each file's `Read` included, arrive on this
/// thread, so a read waiting for bytes from another machine never holds up
/// the clipboard thread. Dropping it takes its copy off the clipboard, if
/// it is still there, without rendering it.
pub(crate) struct Offerer {
    thread: u32,
    offers: Sender<Offer>,
    handle: Option<JoinHandle<()>>,
}

struct Offer {
    entries: Vec<Entry>,
    source: Arc<dyn Source>,
}

/// How long to keep trying to place a copy while another app holds the
/// clipboard.
const PLACE_FOR: Duration = Duration::from_secs(1);

impl Offerer {
    pub(crate) fn start() -> windows_core::Result<Self> {
        let (offers, rx) = mpsc::channel();
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let handle = std::thread::Builder::new()
            .name("deskhop-files".into())
            .spawn(move || offer_thread(&rx, &ready_tx))
            .map_err(windows_core::Error::from)?;
        match ready.recv() {
            Ok(Ok(thread)) => Ok(Self {
                thread,
                offers,
                handle: Some(handle),
            }),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => {
                let _ = handle.join();
                Err(E_OUTOFMEMORY.into())
            }
        }
    }

    /// Places `entries` on the clipboard for paste, their bytes read from
    /// `source` only when a file manager reads them.
    pub(crate) fn offer(&self, entries: Vec<Entry>, source: Arc<dyn Source>) {
        if self.offers.send(Offer { entries, source }).is_ok() {
            // SAFETY: posts a message with no pointers to a thread whose
            // queue exists (it was created before the thread reported ready).
            let _ = unsafe { PostThreadMessageW(self.thread, WM_APP, WPARAM(0), LPARAM(0)) };
        }
    }
}

impl Drop for Offerer {
    fn drop(&mut self) {
        // SAFETY: as in `offer`.
        let _ = unsafe { PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn offer_thread(offers: &Receiver<Offer>, ready: &mpsc::SyncSender<windows_core::Result<u32>>) {
    // SAFETY: once on this new thread, before any COM use; undone below.
    if let Err(e) = unsafe { OleInitialize(None) } {
        let _ = ready.send(Err(e));
        return;
    }
    // SAFETY: no arguments. The queue exists once the thread has run, and
    // GetMessageW below creates it if not.
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
    let mut current: Option<IDataObject> = None;
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid, writable MSG; it is dispatched only after
    // GetMessageW filled it in.
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        if msg.message == WM_APP && msg.hwnd.is_invalid() {
            while let Ok(offer) = offers.try_recv() {
                if let Ok(object) = data_object(offer.entries, offer.source)
                    && place(&object)
                {
                    current = Some(object);
                }
            }
        }
        // SAFETY: as above.
        unsafe { DispatchMessageW(&msg) };
    }
    // Still ours if our marker is still there (OleIsCurrentClipboard's
    // answer, S_FALSE, reads as success through the bindings).
    // SAFETY: a plain format id; the clipboard need not be open.
    if current.is_some() && unsafe { IsClipboardFormatAvailable(own_format()) }.is_ok() {
        // SAFETY: OLE is initialized on this thread, which placed the
        // object; emptying it renders nothing.
        let _ = unsafe { OleSetClipboard(None) };
    }
    drop(current);
    // SAFETY: matches the OleInitialize above, on the same thread.
    unsafe { OleUninitialize() };
}

/// Puts `object` on the clipboard, retrying while another app holds it.
fn place(object: &IDataObject) -> bool {
    let deadline = Instant::now() + PLACE_FOR;
    loop {
        // SAFETY: OLE is initialized on this thread; OLE keeps its own
        // reference to the object while it is on the clipboard.
        if unsafe { OleSetClipboard(object) }.is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use std::mem::{offset_of, size_of};

    use windows::Win32::UI::Shell::FILEDESCRIPTORW;

    use super::*;

    fn entry(path: &str, is_dir: bool, size: u64) -> Entry {
        Entry {
            path: path.into(),
            is_dir,
            size,
        }
    }

    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..][..4].try_into().unwrap())
    }

    fn name_at(fd: &[u8]) -> String {
        let units: Vec<u16> = fd[FD_NAME_AT..][..FD_NAME_UNITS * 2]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u16::from_le_bytes(*p))
            .take_while(|&u| u != 0)
            .collect();
        String::from_utf16(&units).unwrap()
    }

    #[test]
    fn layout_matches_windows() {
        assert_eq!(size_of::<FILEDESCRIPTORW>(), FD_SIZE);
        assert_eq!(offset_of!(FILEDESCRIPTORW, dwFlags), FD_FLAGS_AT);
        assert_eq!(
            offset_of!(FILEDESCRIPTORW, dwFileAttributes),
            FD_ATTRIBUTES_AT
        );
        assert_eq!(offset_of!(FILEDESCRIPTORW, nFileSizeHigh), FD_SIZE_HIGH_AT);
        assert_eq!(offset_of!(FILEDESCRIPTORW, nFileSizeLow), FD_SIZE_LOW_AT);
        assert_eq!(offset_of!(FILEDESCRIPTORW, cFileName), FD_NAME_AT);
    }

    #[test]
    fn descriptor_lists_each_entry() {
        let d = descriptor(&[
            entry("top", true, 0),
            entry("top/big.bin", false, (5 << 32) + 7),
            entry("top/empty", true, 0),
        ])
        .unwrap();
        assert_eq!(d.len(), 4 + 3 * FD_SIZE);
        assert_eq!(u32_at(&d, 0), 3);
        let fd = |i: usize| &d[4 + i * FD_SIZE..][..FD_SIZE];
        assert_eq!(name_at(fd(0)), "top");
        assert_eq!(u32_at(fd(0), FD_ATTRIBUTES_AT), FILE_ATTRIBUTE_DIRECTORY);
        assert_eq!(name_at(fd(1)), "top\\big.bin");
        assert_eq!(u32_at(fd(1), FD_ATTRIBUTES_AT), FILE_ATTRIBUTE_NORMAL);
        assert_eq!(u32_at(fd(1), FD_SIZE_HIGH_AT), 5);
        assert_eq!(u32_at(fd(1), FD_SIZE_LOW_AT), 7);
        assert_eq!(name_at(fd(2)), "top\\empty");
        let flags = u32_at(fd(1), FD_FLAGS_AT);
        for f in [FD_ATTRIBUTES, FD_FILESIZE, FD_PROGRESSUI, FD_UNICODE] {
            assert_ne!(flags & f.0.cast_unsigned(), 0);
        }
    }

    #[test]
    fn descriptor_refuses_what_it_cannot_hold() {
        assert_eq!(descriptor(&[]), Err(DescriptorError::Empty));
        let long = "a/".repeat(129) + "b"; // 259 units: fits
        assert!(descriptor(&[entry(&long, false, 1)]).is_ok());
        let longer = long + "c"; // 260: does not
        assert_eq!(
            descriptor(&[entry("ok", false, 1), entry(&longer, false, 1)]),
            Err(DescriptorError::PathTooLong(1))
        );
    }

    fn etc(cf: u16, lindex: i32, tymed: i32) -> FORMATETC {
        FORMATETC {
            cfFormat: cf,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex,
            tymed: tymed.cast_unsigned(),
        }
    }

    /// A fresh, empty folder under the temp folder.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("deskhop-walk-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn walk_lists_a_tree_with_its_empty_folder() {
        let root = scratch("tree");
        let top = root.join("Photos");
        fs::create_dir_all(top.join("empty")).unwrap();
        fs::create_dir_all(top.join("sub")).unwrap();
        fs::write(top.join("b.jpg"), [0; 7]).unwrap();
        fs::write(top.join("sub").join("a.png"), [0; 3]).unwrap();
        fs::write(root.join("notes.txt"), b"hi").unwrap();
        let copy = walk(&[top.clone(), root.join("notes.txt")], LIMITS).unwrap();
        assert_eq!(
            copy.entries,
            [
                entry("Photos", true, 0),
                entry("Photos/b.jpg", false, 7),
                entry("Photos/empty", true, 0),
                entry("Photos/sub", true, 0),
                entry("Photos/sub/a.png", false, 3),
                entry("notes.txt", false, 2),
            ]
        );
        assert_eq!(copy.sources[4], top.join("sub").join("a.png"));
        assert_eq!(copy.sources[5], root.join("notes.txt"));
    }

    #[test]
    fn walk_keeps_to_the_limits() {
        let root = scratch("limits");
        for i in 0..5 {
            fs::write(root.join(format!("f{i}")), [0; 10]).unwrap();
        }
        let few = Limits {
            entries: 5,
            ..LIMITS
        };
        assert_eq!(
            walk(std::slice::from_ref(&root), few),
            Err(WalkError::TooLarge)
        );
        let small = Limits {
            total: 49,
            ..LIMITS
        };
        assert_eq!(
            walk(std::slice::from_ref(&root), small),
            Err(WalkError::TooLarge)
        );
        let fits = Limits {
            entries: 6,
            total: 50,
            ..LIMITS
        };
        assert_eq!(
            walk(std::slice::from_ref(&root), fits)
                .unwrap()
                .entries
                .len(),
            6
        );
        // The root's own name counts toward a path's length.
        let short = Limits {
            path_units: "deskhop-walk-limits/f0".len() - 1,
            ..LIMITS
        };
        assert_eq!(walk(&[root], short), Err(WalkError::TooLarge));
    }

    #[test]
    fn a_path_over_259_units_makes_the_copy_too_large() {
        let root = scratch("long");
        let deep = root.join("d".repeat(200)).join("e".repeat(70));
        fs::create_dir_all(&deep).unwrap();
        assert_eq!(walk(&[root], LIMITS), Err(WalkError::TooLarge));
    }

    #[test]
    fn walk_of_nothing_or_missing_is_not_shared() {
        assert_eq!(walk(&[], LIMITS), Err(WalkError::Empty));
        let gone = std::env::temp_dir().join("deskhop-walk-missing-xyz");
        let _ = fs::remove_dir_all(&gone);
        assert_eq!(walk(&[gone], LIMITS), Err(WalkError::Unreadable));
    }

    #[test]
    fn formats_are_matched_by_kind_index_and_medium() {
        let formats = Formats {
            descriptor: 0xc001,
            contents: 0xc002,
            effect: 0xc003,
            own: 0xc004,
        };
        let entries = [entry("d", true, 0), entry("d/f", false, 3)];
        let ask = |f| asked(&f, formats, &entries);
        assert_eq!(ask(etc(0xc001, -1, TYMED_HGLOBAL.0)), Ok(Asked::Descriptor));
        assert_eq!(ask(etc(0xc003, -1, TYMED_HGLOBAL.0)), Ok(Asked::Effect));
        assert_eq!(ask(etc(0xc004, -1, TYMED_HGLOBAL.0)), Ok(Asked::Own));
        assert_eq!(ask(etc(0xc002, 1, TYMED_ISTREAM.0)), Ok(Asked::Contents(1)));
        // Several media offered at once.
        assert_eq!(
            ask(etc(0xc002, 1, TYMED_ISTREAM.0 | TYMED_HGLOBAL.0)),
            Ok(Asked::Contents(1))
        );
        assert_eq!(ask(etc(0xc002, 1, TYMED_HGLOBAL.0)), Err(DV_E_TYMED));
        assert_eq!(ask(etc(0xc002, 0, TYMED_ISTREAM.0)), Err(DV_E_LINDEX)); // a directory
        assert_eq!(ask(etc(0xc002, 2, TYMED_ISTREAM.0)), Err(DV_E_LINDEX));
        assert_eq!(ask(etc(0xc002, -1, TYMED_ISTREAM.0)), Err(DV_E_LINDEX));
        assert_eq!(ask(etc(0xc009, -1, TYMED_HGLOBAL.0)), Err(DV_E_FORMATETC));
    }
}

/// The delayed-render spike (add-clipboard-files task 1): against the real
/// OLE clipboard and the shell's copy engine, so they need a logged-on
/// desktop, replace whatever was copied, and must not run in parallel:
///
/// ```text
/// cargo test -p win32-clipboard files::desktop -- --ignored --test-threads=1 --nocapture
/// ```
#[cfg(test)]
mod desktop {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{HWND, LPARAM, POINTL, WPARAM};
    use windows::Win32::System::Ole::{
        IDropTarget, OleGetClipboard, OleInitialize, OleSetClipboard,
    };
    use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Shell::{BHID_SFUIObject, IShellItem, SHCreateItemFromParsingName};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, PM_REMOVE, PeekMessageW, PostThreadMessageW, WM_QUIT,
    };
    use windows_core::HSTRING;

    use super::*;

    /// A file's deterministic bytes, so a pasted copy can be checked.
    fn byte(index: usize, at: u64) -> u8 {
        ((at.wrapping_mul(31) + index as u64 * 7) % 251) as u8
    }

    /// Generated files, optionally slowed to a bytes-per-second rate, as a
    /// network would. Counts the files opened and bytes read.
    struct Pattern {
        sizes: Vec<u64>,
        rate: Option<u64>,
        opened: AtomicUsize,
        read: Arc<AtomicU64>,
    }

    struct PatternReader {
        index: usize,
        at: u64,
        size: u64,
        rate: Option<u64>,
        started: Instant,
        read: Arc<AtomicU64>,
    }

    impl Read for PatternReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = buf
                .len()
                .min(usize::try_from(self.size - self.at).unwrap_or(usize::MAX));
            for (i, b) in buf[..n].iter_mut().enumerate() {
                *b = byte(self.index, self.at + i as u64);
            }
            self.at += n as u64;
            self.read.fetch_add(n as u64, Ordering::Relaxed);
            if let Some(rate) = self.rate {
                let due = Duration::from_secs_f64(self.at as f64 / rate as f64);
                if let Some(wait) = due.checked_sub(self.started.elapsed()) {
                    std::thread::sleep(wait);
                }
            }
            Ok(n)
        }
    }

    impl Source for Pattern {
        fn open(&self, index: usize) -> io::Result<Box<dyn Read + Send>> {
            self.opened.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(PatternReader {
                index,
                at: 0,
                size: self.sizes[index],
                rate: self.rate,
                started: Instant::now(),
                read: self.read.clone(),
            }))
        }
    }

    const MIB: u64 = 1 << 20;

    fn spike_entries(big: u64) -> Vec<Entry> {
        vec![
            Entry {
                path: "DeskHop spike".into(),
                is_dir: true,
                size: 0,
            },
            Entry {
                path: "DeskHop spike/hello.txt".into(),
                is_dir: false,
                size: 20,
            },
            Entry {
                path: "DeskHop spike/empty".into(),
                is_dir: true,
                size: 0,
            },
            Entry {
                path: "DeskHop spike/sub/big.bin".into(),
                is_dir: false,
                size: big,
            },
        ]
    }

    fn pattern(entries: &[Entry], rate: Option<u64>) -> Arc<Pattern> {
        Arc::new(Pattern {
            sizes: entries.iter().map(|e| e.size).collect(),
            rate,
            opened: AtomicUsize::new(0),
            read: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Places `object` on the OLE clipboard from an STA thread that pumps
    /// messages until the returned guard is dropped, as the clipboard
    /// thread will.
    struct Owner {
        thread: u32,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            // SAFETY: posts a message with no pointers to the owner thread.
            let _ = unsafe { PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
            let _ = self.handle.take().map(std::thread::JoinHandle::join);
        }
    }

    fn own(entries: Vec<Entry>, source: Arc<dyn Source>) -> Owner {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let handle = std::thread::spawn(move || {
            // SAFETY: once on this new thread, before any COM use.
            unsafe { OleInitialize(None) }.unwrap();
            let object = data_object(entries, source).unwrap();
            // SAFETY: the object is a valid data object; OLE keeps its own
            // reference while it is on the clipboard.
            unsafe { OleSetClipboard(&object) }.unwrap();
            // SAFETY: no arguments.
            tx.send(unsafe { GetCurrentThreadId() }).unwrap();
            let mut msg = MSG::default();
            // SAFETY: `msg` is a valid, writable MSG; it is dispatched
            // only after GetMessageW filled it in.
            while unsafe { GetMessageW(&mut msg, None::<HWND>, 0, 0) }.0 > 0 {
                // SAFETY: as above.
                unsafe { DispatchMessageW(&msg) };
            }
            // SAFETY: this thread put the object there; empties it again.
            let _ = unsafe { OleSetClipboard(None) };
        });
        Owner {
            thread: rx.recv().unwrap(),
            handle: Some(handle),
        }
    }

    /// Dispatches this thread's messages for `for_`.
    fn pump(for_: Duration) {
        let until = Instant::now() + for_;
        let mut msg = MSG::default();
        while Instant::now() < until {
            // SAFETY: `msg` is a valid, writable MSG; it is dispatched only
            // after PeekMessageW filled it in.
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                // SAFETY: as above.
                unsafe { DispatchMessageW(&msg) };
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn check_file(path: &Path, index: usize, size: u64) {
        let got = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(got.len() as u64, size, "{}", path.display());
        let bad = got
            .iter()
            .enumerate()
            .position(|(i, b)| *b != byte(index, i as u64));
        assert_eq!(bad, None, "{} differs", path.display());
    }

    fn fresh_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Reads the clipboard back through OLE, as a pasting app would, from
    /// another STA thread: the descriptor, then one file's stream to its
    /// end. Nothing is opened before the stream is asked for.
    #[test]
    #[ignore = "uses the desktop clipboard"]
    fn files_read_back_through_the_ole_clipboard() {
        let entries = spike_entries(3 * MIB + 5);
        let source = pattern(&entries, None);
        let _owner = own(entries.clone(), source.clone());
        // SAFETY: once on this test thread, before any COM use.
        unsafe { OleInitialize(None) }.unwrap();
        // SAFETY: OLE is initialized on this thread.
        let object = unsafe { OleGetClipboard() }.unwrap();
        let formats = Formats::registered();

        let mut f = FORMATETC {
            cfFormat: formats.descriptor,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0.cast_unsigned(),
        };
        // SAFETY: `f` is valid for the call; the medium is ours to read and
        // release.
        let mut medium = unsafe { object.GetData(&f) }.unwrap();
        // SAFETY: an HGLOBAL medium holds the descriptor; locked to read.
        let got = unsafe {
            let g = medium.u.hGlobal;
            let p = GlobalLock(g).cast::<u8>();
            let bytes = std::slice::from_raw_parts(p, descriptor(&entries).unwrap().len()).to_vec();
            let _ = GlobalUnlock(g);
            ReleaseStgMedium(&mut medium);
            bytes
        };
        assert_eq!(got, descriptor(&entries).unwrap());
        assert_eq!(
            source.opened.load(Ordering::Relaxed),
            0,
            "opened before paste"
        );

        f.cfFormat = formats.contents;
        f.lindex = 3;
        f.tymed = TYMED_ISTREAM.0.cast_unsigned();
        // SAFETY: as above.
        let mut medium = unsafe { object.GetData(&f) }.unwrap();
        // SAFETY: an ISTREAM medium holds a stream; cloned out before the
        // medium is released.
        let stream = unsafe { (*medium.u.pstm).clone().unwrap() };
        // SAFETY: the medium is ours and released once.
        unsafe { ReleaseStgMedium(&mut medium) };
        let mut stat = STATSTG::default();
        // SAFETY: `stat` is writable.
        unsafe { stream.Stat(&mut stat, windows::Win32::System::Com::STATFLAG_NONAME) }.unwrap();
        assert_eq!(stat.cbSize, 3 * MIB + 5);
        let mut all = Vec::new();
        let mut buf = vec![0u8; 100_000];
        loop {
            let mut n = 0u32;
            // SAFETY: `buf` is writable for its length.
            let hr =
                unsafe { stream.Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut n)) };
            assert!(hr.is_ok(), "{hr:?}");
            all.extend_from_slice(&buf[..n as usize]);
            if n == 0 {
                break;
            }
        }
        assert_eq!(all.len() as u64, 3 * MIB + 5);
        assert!(all.iter().enumerate().all(|(i, b)| *b == byte(3, i as u64)));
    }

    /// Pastes as Explorer's Paste does, by dropping the data object on the
    /// target folder's own drop target, into a fresh folder: the tree is
    /// created, the empty folder too, and each file's bytes arrive as the
    /// source produces them.
    #[test]
    #[ignore = "uses the shell"]
    fn files_paste_through_the_folder_drop_target() {
        let big = 64 * MIB + 3;
        let entries = spike_entries(big);
        let source = pattern(&entries, None);
        let object = {
            // SAFETY: once on this test thread, before any COM use.
            unsafe { OleInitialize(None) }.unwrap();
            data_object(entries, source.clone()).unwrap()
        };
        let target = fresh_dir("deskhop-spike-drop");
        // SAFETY: COM is initialized; the path is a valid string, and the
        // object and effect live for the calls.
        unsafe {
            let folder: IShellItem =
                SHCreateItemFromParsingName(&HSTRING::from(target.as_os_str()), None).unwrap();
            let drop: IDropTarget = folder.BindToHandler(None, &BHID_SFUIObject).unwrap();
            let mut effect = DROPEFFECT_COPY;
            let keys = MODIFIERKEYS_FLAGS(0);
            drop.DragEnter(&object, keys, POINTL::default(), &mut effect)
                .unwrap();
            assert_eq!(effect, DROPEFFECT_COPY, "the folder will not take a copy");
            drop.Drop(&object, keys, POINTL::default(), &mut effect)
                .unwrap();
        }
        // The shell may copy on a thread of its own after Drop returns;
        // keep this thread's messages moving until the bytes are all read.
        let deadline = Instant::now() + Duration::from_secs(60);
        while source.read.load(Ordering::Relaxed) < big + 20 {
            assert!(Instant::now() < deadline, "the paste never finished");
            pump(Duration::from_millis(50));
        }
        pump(Duration::from_secs(2));
        let root = target.join("DeskHop spike");
        check_file(&root.join("hello.txt"), 1, 20);
        check_file(&root.join("sub").join("big.bin"), 3, big);
        assert!(root.join("empty").is_dir(), "empty folder not created");
        assert_eq!(source.opened.load(Ordering::Relaxed), 2);
    }

    /// Drops `object` on a fresh folder as Explorer's Paste does and waits
    /// until `source` has produced `want` bytes; the folder.
    fn paste_into(object: &IDataObject, source: &Pattern, want: u64, name: &str) -> PathBuf {
        let target = fresh_dir(name);
        // SAFETY: COM is initialized on this thread; the path is a valid
        // string, and the object and effect live for the calls.
        unsafe {
            let folder: IShellItem =
                SHCreateItemFromParsingName(&HSTRING::from(target.as_os_str()), None).unwrap();
            let drop: IDropTarget = folder.BindToHandler(None, &BHID_SFUIObject).unwrap();
            let mut effect = DROPEFFECT_COPY;
            let keys = MODIFIERKEYS_FLAGS(0);
            drop.DragEnter(object, keys, POINTL::default(), &mut effect)
                .unwrap();
            drop.Drop(object, keys, POINTL::default(), &mut effect)
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        while source.read.load(Ordering::Relaxed) < want {
            assert!(Instant::now() < deadline, "the paste never finished");
            pump(Duration::from_millis(50));
        }
        pump(Duration::from_secs(2));
        target
    }

    /// Files offered through the clipboard thread's `set_files` go on the
    /// clipboard from the files thread, are never reported as a copy, and
    /// paste, reading each file only then.
    #[test]
    #[ignore = "uses the desktop clipboard and the shell"]
    fn offered_files_paste_and_are_not_reported() {
        let (clipboard, rx) = crate::start().unwrap();
        let big = 3 * MIB + 1;
        let entries = spike_entries(big);
        let source = pattern(&entries, None);
        assert!(clipboard.set_files(entries, source.clone()));
        if let Ok(c) = rx.recv_timeout(Duration::from_millis(600)) {
            panic!("offered files reported as {c:?}");
        }
        assert_eq!(
            source.opened.load(Ordering::Relaxed),
            0,
            "read before paste"
        );
        // SAFETY: once on this test thread, before any COM use.
        unsafe { OleInitialize(None) }.unwrap();
        // SAFETY: OLE is initialized on this thread.
        let object = unsafe { OleGetClipboard() }.unwrap();
        let root =
            paste_into(&object, &source, big + 20, "deskhop-offered-paste").join("DeskHop spike");
        check_file(&root.join("hello.txt"), 1, 20);
        check_file(&root.join("sub").join("big.bin"), 3, big);
        assert!(root.join("empty").is_dir(), "empty folder not created");
        // Dropping the clipboard takes the offer off it.
        drop(object);
        drop(clipboard);
        // SAFETY: a plain format id.
        assert!(unsafe { IsClipboardFormatAvailable(own_format()) }.is_err());
    }

    /// By hand: places a slowed 256 MiB file and a small tree on the
    /// clipboard for 10 minutes. Paste it into `%TEMP%\deskhop-spike-paste`
    /// in Explorer (Ctrl+V); Explorer's progress dialog should show. The
    /// test then checks what landed there.
    #[test]
    #[ignore = "needs a person to paste in Explorer"]
    fn files_paste_in_explorer_by_hand() {
        let big = 256 * MIB;
        let entries = spike_entries(big);
        let source = pattern(&entries, Some(32 * MIB));
        let target = fresh_dir("deskhop-spike-paste");
        let owner = own(entries, source.clone());
        eprintln!(
            "On the clipboard now. In Explorer, open {} and press Ctrl+V.",
            target.display()
        );
        let deadline = Instant::now() + Duration::from_secs(600);
        let root = target.join("DeskHop spike");
        while source.read.load(Ordering::Relaxed) < big + 20 {
            assert!(Instant::now() < deadline, "nothing was pasted in time");
            std::thread::sleep(Duration::from_millis(250));
        }
        // Let Explorer close the last file.
        std::thread::sleep(Duration::from_secs(2));
        drop(owner);
        check_file(&root.join("hello.txt"), 1, 20);
        check_file(&root.join("sub").join("big.bin"), 3, big);
        assert!(root.join("empty").is_dir(), "empty folder not created");
        eprintln!("Pasted and checked: {}", root.display());
    }
}
