//! `Hello`, the message registry, and the agent link's messages (IPC
//! versions 1 to 3; version 2 adds the image runs, version 3 the file
//! messages, `0x0040`–`0x004D`).
//!
//! All integers are little-endian and every payload but `Screen` has a fixed
//! length. Times are milliseconds on the sender's monotonic clock.

use model::{
    Button, CaptureMode, InputAction, InputEvent, Key, Millis, Monitor, MonitorId, Origin, Point,
    Rect, Screen,
};

use crate::clipboard::{MAX_IMAGE_LEN, MAX_TEXT_LEN, NotShared};
use crate::files::MAX_MANIFEST_LEN;
use crate::{EncodeError, IPC_VERSION, MIN_IPC_VERSION, ProtocolError, frame, ui};

/// Never a valid message type.
pub const RESERVED: u16 = 0x0000;
/// Version and role. The first frame in each direction.
pub const HELLO: u16 = 0x0001;

pub const KEY_EVENT: u16 = 0x0010;
pub const BUTTON_EVENT: u16 = 0x0011;
pub const WHEEL_EVENT: u16 = 0x0012;
pub const MOTION_EVENT: u16 = 0x0013;
pub const SCREEN: u16 = 0x0014;
pub const PONG: u16 = 0x0015;
/// Starts a run of copied text: its length in bytes of UTF-8.
pub const CLIPBOARD_TEXT: u16 = 0x0016;
/// Copied text bytes, within a `ClipboardText` run.
pub const CLIPBOARD_CHUNK: u16 = 0x0017;
/// The newest copy is not shared, and why.
pub const CLIPBOARD_NOT_SHARED: u16 = 0x0018;
/// Whether the winlogon agent's desktop is the input desktop.
pub const INPUT_DESKTOP: u16 = 0x0019;
/// Starts a run of a copied image: its length in bytes of PNG. Version 2.
pub const CLIPBOARD_IMAGE: u16 = 0x001a;

pub const SET_CAPTURE: u16 = 0x0020;
pub const INJECT_KEY: u16 = 0x0021;
pub const INJECT_BUTTON: u16 = 0x0022;
pub const INJECT_WHEEL: u16 = 0x0023;
pub const INJECT_MOTION: u16 = 0x0024;
pub const WARP: u16 = 0x0025;
pub const PING: u16 = 0x0026;
/// Starts a run of text for the agent to put on the clipboard.
pub const SET_CLIPBOARD: u16 = 0x0027;
/// Bytes, within a `SetClipboard` or `SetClipboardImage` run.
pub const SET_CLIPBOARD_CHUNK: u16 = 0x0028;
/// Starts a run of an image, as one PNG, for the agent to put on the
/// clipboard. Version 2.
pub const SET_CLIPBOARD_IMAGE: u16 = 0x0029;

/// Starts a run of a copied file copy's manifest: its length. Version 3.
pub const CLIPBOARD_FILES: u16 = 0x0040;
/// The receiving agent wants one file of a file copy it was set. Version 3.
pub const FILE_WANTED: u16 = 0x0041;
/// The owning agent starts serving one file: its index and `u64` length.
/// Version 3.
pub const FILE_CONTENT: u16 = 0x0042;
/// Bytes of the file being served, within a `FileContent` run. Version 3.
pub const FILE_CHUNK: u16 = 0x0043;
/// The owning agent stops serving its file: its run ends. Version 3.
pub const FILE_ABANDONED: u16 = 0x0044;
/// The receiving agent passed on this many more bytes of a file being set.
/// Version 3.
pub const FILE_CONSUMED: u16 = 0x0045;
/// The receiving agent no longer wants the file being set. Version 3.
pub const FILE_UNWANTED: u16 = 0x0046;
/// Starts a run of a file copy's manifest for the agent to offer for paste.
/// Version 3.
pub const SET_CLIPBOARD_FILES: u16 = 0x0048;
/// Asks the owning agent to serve one file of a copy it reported. Version 3.
pub const SEND_FILE: u16 = 0x0049;
/// A wanted file's bytes start: its index and `u64` length. Version 3.
pub const SET_FILE_CONTENT: u16 = 0x004a;
/// Bytes of the file being set, within a `SetFileContent` run. Version 3.
pub const SET_FILE_CHUNK: u16 = 0x004b;
/// The file being set will not complete: its run ends. Version 3.
pub const ABANDON_FILE: u16 = 0x004c;
/// The service passed on this many more bytes of the file being served.
/// Version 3.
pub const FILE_ACKED: u16 = 0x004d;
/// The owning agent should stop serving its file. Version 3.
pub const STOP_FILE: u16 = 0x004e;

/// Most monitors a `Screen` may list.
pub const MAX_MONITORS: usize = 32;
/// Longest monitor identity, in bytes of UTF-8.
pub const MAX_ID_LEN: usize = 256;
/// Lowest and highest plausible effective DPI.
pub const DPI_RANGE: std::ops::RangeInclusive<u32> = 48..=960;

/// Which way a message type may travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From an agent to the service.
    AgentToService,
    /// From the service to an agent.
    ServiceToAgent,
    /// From a UI to the service.
    UiToService,
    /// From the service to a UI.
    ServiceToUi,
    /// Either way, on every link (`Hello`).
    Both,
}

/// One registered message type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeInfo {
    pub ty: u16,
    pub name: &'static str,
    pub direction: Direction,
    /// IPC version that introduced this type.
    pub since: u16,
}

const fn info(ty: u16, name: &'static str, direction: Direction) -> TypeInfo {
    TypeInfo {
        ty,
        name,
        direction,
        since: 1,
    }
}

/// A message type introduced in IPC version 2.
const fn info_v2(ty: u16, name: &'static str, direction: Direction) -> TypeInfo {
    TypeInfo {
        ty,
        name,
        direction,
        since: 2,
    }
}

/// A message type introduced in IPC version 3.
const fn info_v3(ty: u16, name: &'static str, direction: Direction) -> TypeInfo {
    TypeInfo {
        ty,
        name,
        direction,
        since: 3,
    }
}

/// Every message type, in type order.
pub const REGISTRY: &[TypeInfo] = &[
    info(HELLO, "Hello", Direction::Both),
    info(KEY_EVENT, "KeyEvent", Direction::AgentToService),
    info(BUTTON_EVENT, "ButtonEvent", Direction::AgentToService),
    info(WHEEL_EVENT, "WheelEvent", Direction::AgentToService),
    info(MOTION_EVENT, "MotionEvent", Direction::AgentToService),
    info(SCREEN, "Screen", Direction::AgentToService),
    info(PONG, "Pong", Direction::AgentToService),
    info(CLIPBOARD_TEXT, "ClipboardText", Direction::AgentToService),
    info(CLIPBOARD_CHUNK, "ClipboardChunk", Direction::AgentToService),
    info(
        CLIPBOARD_NOT_SHARED,
        "ClipboardNotShared",
        Direction::AgentToService,
    ),
    info(INPUT_DESKTOP, "InputDesktop", Direction::AgentToService),
    info_v2(CLIPBOARD_IMAGE, "ClipboardImage", Direction::AgentToService),
    info(SET_CAPTURE, "SetCapture", Direction::ServiceToAgent),
    info(INJECT_KEY, "InjectKey", Direction::ServiceToAgent),
    info(INJECT_BUTTON, "InjectButton", Direction::ServiceToAgent),
    info(INJECT_WHEEL, "InjectWheel", Direction::ServiceToAgent),
    info(INJECT_MOTION, "InjectMotion", Direction::ServiceToAgent),
    info(WARP, "Warp", Direction::ServiceToAgent),
    info(PING, "Ping", Direction::ServiceToAgent),
    info(SET_CLIPBOARD, "SetClipboard", Direction::ServiceToAgent),
    info(
        SET_CLIPBOARD_CHUNK,
        "SetClipboardChunk",
        Direction::ServiceToAgent,
    ),
    info_v2(
        SET_CLIPBOARD_IMAGE,
        "SetClipboardImage",
        Direction::ServiceToAgent,
    ),
    info(ui::OPEN_PAIRING, "OpenPairing", Direction::UiToService),
    info(ui::CLOSE_PAIRING, "ClosePairing", Direction::UiToService),
    info(ui::JOIN, "Join", Direction::UiToService),
    info(ui::REMOVE_PEER, "RemovePeer", Direction::UiToService),
    info(ui::UNDO, "Undo", Direction::UiToService),
    info(ui::FORGET_LAYOUT, "ForgetLayout", Direction::UiToService),
    info(ui::STATUS, "Status", Direction::ServiceToUi),
    info(ui::PAIRED, "Paired", Direction::ServiceToUi),
    info(ui::JOIN_FAILED, "JoinFailed", Direction::ServiceToUi),
    info(ui::PAIRING_CLOSED, "PairingClosed", Direction::ServiceToUi),
    info(
        ui::PAIRING_NOT_OPENED,
        "PairingNotOpened",
        Direction::ServiceToUi,
    ),
    info(ui::LEARNED, "Learned", Direction::ServiceToUi),
    info_v3(CLIPBOARD_FILES, "ClipboardFiles", Direction::AgentToService),
    info_v3(FILE_WANTED, "FileWanted", Direction::AgentToService),
    info_v3(FILE_CONTENT, "FileContent", Direction::AgentToService),
    info_v3(FILE_CHUNK, "FileChunk", Direction::AgentToService),
    info_v3(FILE_ABANDONED, "FileAbandoned", Direction::AgentToService),
    info_v3(FILE_CONSUMED, "FileConsumed", Direction::AgentToService),
    info_v3(FILE_UNWANTED, "FileUnwanted", Direction::AgentToService),
    info_v3(
        SET_CLIPBOARD_FILES,
        "SetClipboardFiles",
        Direction::ServiceToAgent,
    ),
    info_v3(SEND_FILE, "SendFile", Direction::ServiceToAgent),
    info_v3(
        SET_FILE_CONTENT,
        "SetFileContent",
        Direction::ServiceToAgent,
    ),
    info_v3(SET_FILE_CHUNK, "SetFileChunk", Direction::ServiceToAgent),
    info_v3(ABANDON_FILE, "AbandonFile", Direction::ServiceToAgent),
    info_v3(FILE_ACKED, "FileAcked", Direction::ServiceToAgent),
    info_v3(STOP_FILE, "StopFile", Direction::ServiceToAgent),
];

/// Checks that `ty` is defined in `version` and may arrive at an end that
/// receives `receiving`.
pub fn check(
    ty: u16,
    receiving: Direction,
    version: u16,
) -> Result<&'static TypeInfo, ProtocolError> {
    if ty == RESERVED {
        return Err(ProtocolError::ReservedType);
    }
    let info = REGISTRY
        .iter()
        .find(|i| i.ty == ty && i.since <= version)
        .ok_or(ProtocolError::UnknownType { ty })?;
    if info.direction != Direction::Both && info.direction != receiving {
        return Err(ProtocolError::WrongDirection { ty });
    }
    Ok(info)
}

/// Checks that an agent with `role` may send `ty`: the clipboard types only
/// from the session agent, `InputDesktop` only from the winlogon agent.
pub fn check_agent_role(ty: u16, role: Role) -> Result<(), ProtocolError> {
    let allowed = match ty {
        CLIPBOARD_TEXT | CLIPBOARD_IMAGE | CLIPBOARD_CHUNK | CLIPBOARD_NOT_SHARED
        | CLIPBOARD_FILES | FILE_WANTED | FILE_CONTENT | FILE_CHUNK | FILE_ABANDONED
        | FILE_CONSUMED | FILE_UNWANTED => role == Role::SessionAgent,
        INPUT_DESKTOP => role == Role::WinlogonAgent,
        _ => true,
    };
    if allowed {
        Ok(())
    } else {
        Err(ProtocolError::WrongRole {
            ty,
            role: role.code(),
        })
    }
}

// ---- Hello ----------------------------------------------------------------

/// What an end is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    SessionAgent,
    WinlogonAgent,
    Ui,
    Service,
}

impl Role {
    pub const fn code(self) -> u8 {
        match self {
            Self::SessionAgent => 1,
            Self::WinlogonAgent => 2,
            Self::Ui => 3,
            Self::Service => 4,
        }
    }

    pub const fn from_code(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::SessionAgent),
            2 => Some(Self::WinlogonAgent),
            3 => Some(Self::Ui),
            4 => Some(Self::Service),
            _ => None,
        }
    }
}

/// `Hello`: the sender's IPC version range and role. Bytes after these
/// fields are ignored, so later versions can extend it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hello {
    pub min: u16,
    pub max: u16,
    pub role: Role,
}

const HELLO_LEN: usize = 5;

impl Hello {
    /// This release's `Hello` for `role`.
    pub const fn this_release(role: Role) -> Self {
        Self {
            min: MIN_IPC_VERSION,
            max: IPC_VERSION,
            role,
        }
    }

    pub fn payload(&self) -> [u8; HELLO_LEN] {
        let [a, b] = self.min.to_le_bytes();
        let [c, d] = self.max.to_le_bytes();
        [a, b, c, d, self.role.code()]
    }

    pub fn encode_frame(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        frame::encode(HELLO, &self.payload(), out)
    }

    pub fn decode(payload: &[u8]) -> Result<Self, ProtocolError> {
        let Some(&[a, b, c, d, role]) = payload.first_chunk::<HELLO_LEN>() else {
            return Err(ProtocolError::PayloadLength {
                ty: HELLO,
                expected: HELLO_LEN,
                actual: payload.len(),
            });
        };
        let (min, max) = (u16::from_le_bytes([a, b]), u16::from_le_bytes([c, d]));
        if min == 0 || min > max {
            return Err(ProtocolError::InvalidVersionRange { min, max });
        }
        let role = Role::from_code(role).ok_or(ProtocolError::InvalidCode {
            ty: HELLO,
            value: role,
        })?;
        Ok(Self { min, max, role })
    }

    /// The version both ends use, if their ranges overlap.
    pub fn agree(&self, other: &Hello) -> Option<u16> {
        let low = self.min.max(other.min);
        let high = self.max.min(other.max);
        (low <= high).then_some(high)
    }
}

// ---- codes ----------------------------------------------------------------

const fn origin_code(o: Origin) -> u8 {
    match o {
        Origin::Physical => 0,
        Origin::Injected => 1,
    }
}

const fn origin_from(ty: u16, value: u8) -> Result<Origin, ProtocolError> {
    match value {
        0 => Ok(Origin::Physical),
        1 => Ok(Origin::Injected),
        _ => Err(ProtocolError::InvalidCode { ty, value }),
    }
}

const fn down_code(down: bool) -> u8 {
    if down { 1 } else { 0 }
}

const fn down_from(ty: u16, value: u8) -> Result<bool, ProtocolError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(ProtocolError::InvalidCode { ty, value }),
    }
}

const fn button_code(b: Button) -> u8 {
    match b {
        Button::Left => 0,
        Button::Right => 1,
        Button::Middle => 2,
        Button::X1 => 3,
        Button::X2 => 4,
    }
}

const fn button_from(ty: u16, value: u8) -> Result<Button, ProtocolError> {
    match value {
        0 => Ok(Button::Left),
        1 => Ok(Button::Right),
        2 => Ok(Button::Middle),
        3 => Ok(Button::X1),
        4 => Ok(Button::X2),
        _ => Err(ProtocolError::InvalidCode { ty, value }),
    }
}

const fn mode_code(m: CaptureMode) -> u8 {
    match m {
        CaptureMode::PassAll => 0,
        CaptureMode::WithholdAll => 1,
        CaptureMode::WithholdMouse => 2,
    }
}

const fn mode_from(ty: u16, value: u8) -> Result<CaptureMode, ProtocolError> {
    match value {
        0 => Ok(CaptureMode::PassAll),
        1 => Ok(CaptureMode::WithholdAll),
        2 => Ok(CaptureMode::WithholdMouse),
        _ => Err(ProtocolError::InvalidCode { ty, value }),
    }
}

// ---- field reading ----------------------------------------------------------

/// Reads fields front to back; every shortfall is a payload-length error.
pub(crate) struct Fields<'a> {
    ty: u16,
    rest: &'a [u8],
    len: usize,
}

impl<'a> Fields<'a> {
    pub(crate) const fn new(ty: u16, payload: &'a [u8]) -> Self {
        Self {
            ty,
            rest: payload,
            len: payload.len(),
        }
    }

    pub(crate) const fn ty(&self) -> u16 {
        self.ty
    }

    fn short(&self) -> ProtocolError {
        ProtocolError::PayloadLength {
            ty: self.ty,
            expected: self.len.saturating_add(1),
            actual: self.len,
        }
    }

    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        let (head, rest) = self.rest.split_first_chunk::<N>().ok_or(self.short())?;
        self.rest = rest;
        Ok(*head)
    }

    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8], ProtocolError> {
        let (head, rest) = self.rest.split_at_checked(n).ok_or(self.short())?;
        self.rest = rest;
        Ok(head)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, ProtocolError> {
        self.array::<1>().map(|[b]| b)
    }

    pub(crate) fn u16(&mut self) -> Result<u16, ProtocolError> {
        self.array().map(u16::from_le_bytes)
    }

    pub(crate) fn u32(&mut self) -> Result<u32, ProtocolError> {
        self.array().map(u32::from_le_bytes)
    }

    fn i32(&mut self) -> Result<i32, ProtocolError> {
        self.array().map(i32::from_le_bytes)
    }

    fn u64(&mut self) -> Result<u64, ProtocolError> {
        self.array().map(u64::from_le_bytes)
    }

    /// Fails unless every byte was read.
    pub(crate) fn end(&self) -> Result<(), ProtocolError> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(ProtocolError::PayloadLength {
                ty: self.ty,
                expected: self.len.saturating_sub(self.rest.len()),
                actual: self.len,
            })
        }
    }
}

/// Checks that `payload` is exactly `n` bytes.
pub(crate) const fn exactly(ty: u16, payload: &[u8], n: usize) -> Result<(), ProtocolError> {
    if payload.len() == n {
        Ok(())
    } else {
        Err(ProtocolError::PayloadLength {
            ty,
            expected: n,
            actual: payload.len(),
        })
    }
}

// ---- screen limits ----------------------------------------------------------

/// True if a monitor's rectangle and DPI are within the limits a receiver
/// enforces.
fn plausible(rect: &Rect, dpi: u32) -> bool {
    rect.w >= 1
        && rect.h >= 1
        && DPI_RANGE.contains(&dpi)
        && rect.x.checked_add(rect.w).is_some()
        && rect.y.checked_add(rect.h).is_some()
}

// ---- agent → service ------------------------------------------------------

/// A message from an agent to the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToService {
    /// Captured input: a key, button, wheel or motion event.
    Input {
        at: Millis,
        input: InputEvent,
        origin: Origin,
    },
    Screen(Screen),
    /// The answer to `Ping`: the service's time from it and the agent's.
    Pong {
        service: Millis,
        agent: Millis,
    },
    /// A run of copied text starts: its length in bytes of UTF-8.
    ClipboardText(u32),
    /// A run of a copied image starts: its length in bytes of PNG. Version 2.
    ClipboardImage(u32),
    /// Copied bytes within the run; 1 to 65,536 of them.
    ClipboardChunk(Vec<u8>),
    /// The newest copy is not shared.
    ClipboardNotShared(NotShared),
    /// Whether the agent's own desktop is the input desktop. Winlogon
    /// agent only.
    InputDesktop(bool),
    /// A run of a copied file copy's manifest starts: its length. Version 3.
    ClipboardFiles(u32),
    /// The agent wants file `index` of the file copy it was set as number
    /// `set`. Version 3.
    FileWanted {
        set: u32,
        index: u32,
    },
    /// The agent starts serving file `index` of its copy: `len` bytes
    /// follow in `FileChunk`s. Version 3.
    FileContent {
        index: u32,
        len: u64,
    },
    /// Bytes of the file being served; 1 to 65,536 of them. Version 3.
    FileChunk(Vec<u8>),
    /// The agent stops serving file `index`, ending its run. Version 3.
    FileAbandoned {
        index: u32,
    },
    /// The agent passed on this many more bytes of the file being set.
    /// Version 3.
    FileConsumed(u32),
    /// The agent no longer wants file `index`, being set. Version 3.
    FileUnwanted {
        index: u32,
    },
}

impl ToService {
    pub fn ty(&self) -> u16 {
        match self {
            Self::Input { input, .. } => match input {
                InputEvent::Key { .. } => KEY_EVENT,
                InputEvent::Button { .. } => BUTTON_EVENT,
                InputEvent::Wheel { .. } => WHEEL_EVENT,
                InputEvent::Motion { .. } => MOTION_EVENT,
            },
            Self::Screen(_) => SCREEN,
            Self::Pong { .. } => PONG,
            Self::ClipboardText(_) => CLIPBOARD_TEXT,
            Self::ClipboardImage(_) => CLIPBOARD_IMAGE,
            Self::ClipboardChunk(_) => CLIPBOARD_CHUNK,
            Self::ClipboardNotShared(_) => CLIPBOARD_NOT_SHARED,
            Self::InputDesktop(_) => INPUT_DESKTOP,
            Self::ClipboardFiles(_) => CLIPBOARD_FILES,
            Self::FileWanted { .. } => FILE_WANTED,
            Self::FileContent { .. } => FILE_CONTENT,
            Self::FileChunk(_) => FILE_CHUNK,
            Self::FileAbandoned { .. } => FILE_ABANDONED,
            Self::FileConsumed(_) => FILE_CONSUMED,
            Self::FileUnwanted { .. } => FILE_UNWANTED,
        }
    }

    /// The payload bytes, or an error for a screen outside the limits.
    pub fn payload(&self) -> Result<Vec<u8>, EncodeError> {
        let mut p = Vec::new();
        match self {
            Self::Input { at, input, origin } => {
                p.extend_from_slice(&at.0.to_le_bytes());
                p.push(origin_code(*origin));
                match *input {
                    InputEvent::Key { key, down } => {
                        p.extend_from_slice(&key.0.to_le_bytes());
                        p.push(down_code(down));
                    }
                    InputEvent::Button { button, down } => {
                        p.push(button_code(button));
                        p.push(down_code(down));
                    }
                    InputEvent::Wheel { dx, dy } => {
                        p.extend_from_slice(&dx.to_le_bytes());
                        p.extend_from_slice(&dy.to_le_bytes());
                    }
                    InputEvent::Motion { dx, dy, cursor } => {
                        for v in [dx, dy, cursor.x, cursor.y] {
                            p.extend_from_slice(&v.to_le_bytes());
                        }
                    }
                }
            }
            Self::Screen(screen) => {
                let n = u8::try_from(screen.monitors.len())
                    .ok()
                    .filter(|n| usize::from(*n) <= MAX_MONITORS)
                    .ok_or(EncodeError::InvalidScreen)?;
                p.push(n);
                for m in &screen.monitors {
                    let id = m.id.0.as_bytes();
                    if id.is_empty() || id.len() > MAX_ID_LEN || !plausible(&m.rect, m.dpi) {
                        return Err(EncodeError::InvalidScreen);
                    }
                    let len = u16::try_from(id.len()).map_err(|_| EncodeError::InvalidScreen)?;
                    p.extend_from_slice(&len.to_le_bytes());
                    p.extend_from_slice(id);
                    for v in [m.rect.x, m.rect.y, m.rect.w, m.rect.h] {
                        p.extend_from_slice(&v.to_le_bytes());
                    }
                    p.extend_from_slice(&m.dpi.to_le_bytes());
                }
            }
            Self::Pong { service, agent } => {
                p.extend_from_slice(&service.0.to_le_bytes());
                p.extend_from_slice(&agent.0.to_le_bytes());
            }
            Self::ClipboardText(len) => p.extend_from_slice(&text_len(*len)?.to_le_bytes()),
            Self::ClipboardImage(len) => p.extend_from_slice(&image_len(*len)?.to_le_bytes()),
            Self::ClipboardChunk(bytes) => p.extend_from_slice(chunk(bytes)?),
            Self::ClipboardNotShared(why) => p.push(why.code()),
            Self::InputDesktop(active) => p.push(u8::from(*active)),
            Self::ClipboardFiles(len) => p.extend_from_slice(&manifest_len(*len)?.to_le_bytes()),
            Self::FileWanted { set, index } => {
                p.extend_from_slice(&set.to_le_bytes());
                p.extend_from_slice(&index.to_le_bytes());
            }
            Self::FileContent { index, len } => {
                p.extend_from_slice(&index.to_le_bytes());
                p.extend_from_slice(&len.to_le_bytes());
            }
            Self::FileChunk(bytes) => p.extend_from_slice(chunk(bytes)?),
            Self::FileAbandoned { index } | Self::FileUnwanted { index } => {
                p.extend_from_slice(&index.to_le_bytes())
            }
            Self::FileConsumed(n) => p.extend_from_slice(&n.to_le_bytes()),
        }
        Ok(p)
    }

    pub fn encode_frame(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        frame::encode(self.ty(), &self.payload()?, out)
    }

    /// Decodes a message the service received, after `Hello`.
    pub fn decode(ty: u16, payload: &[u8], version: u16) -> Result<Self, ProtocolError> {
        check(ty, Direction::AgentToService, version)?;
        let mut f = Fields::new(ty, payload);
        let input = |f: &mut Fields<'_>, n: usize| -> Result<(Millis, Origin), ProtocolError> {
            exactly(ty, payload, n)?;
            let at = Millis(f.u64()?);
            let origin = origin_from(ty, f.u8()?)?;
            Ok((at, origin))
        };
        let message = match ty {
            HELLO => return Err(ProtocolError::DuplicateHello),
            KEY_EVENT => {
                let (at, origin) = input(&mut f, 12)?;
                let key = Key(f.u16()?);
                let down = down_from(ty, f.u8()?)?;
                Self::Input {
                    at,
                    input: InputEvent::Key { key, down },
                    origin,
                }
            }
            BUTTON_EVENT => {
                let (at, origin) = input(&mut f, 11)?;
                let button = button_from(ty, f.u8()?)?;
                let down = down_from(ty, f.u8()?)?;
                Self::Input {
                    at,
                    input: InputEvent::Button { button, down },
                    origin,
                }
            }
            WHEEL_EVENT => {
                let (at, origin) = input(&mut f, 17)?;
                let (dx, dy) = (f.i32()?, f.i32()?);
                Self::Input {
                    at,
                    input: InputEvent::Wheel { dx, dy },
                    origin,
                }
            }
            MOTION_EVENT => {
                let (at, origin) = input(&mut f, 25)?;
                let (dx, dy) = (f.i32()?, f.i32()?);
                let cursor = Point::new(f.i32()?, f.i32()?);
                Self::Input {
                    at,
                    input: InputEvent::Motion { dx, dy, cursor },
                    origin,
                }
            }
            SCREEN => Self::Screen(decode_screen(&mut f)?),
            PONG => {
                exactly(ty, payload, 16)?;
                Self::Pong {
                    service: Millis(f.u64()?),
                    agent: Millis(f.u64()?),
                }
            }
            CLIPBOARD_TEXT => Self::ClipboardText(decode_text_len(ty, payload, &mut f)?),
            CLIPBOARD_IMAGE => Self::ClipboardImage(decode_image_len(ty, payload, &mut f)?),
            CLIPBOARD_CHUNK => Self::ClipboardChunk(decode_chunk(ty, payload, &mut f)?),
            CLIPBOARD_NOT_SHARED => {
                exactly(ty, payload, 1)?;
                Self::ClipboardNotShared(NotShared::from_code(ty, f.u8()?)?)
            }
            INPUT_DESKTOP => {
                exactly(ty, payload, 1)?;
                Self::InputDesktop(match f.u8()? {
                    0 => false,
                    1 => true,
                    value => return Err(ProtocolError::InvalidCode { ty, value }),
                })
            }
            CLIPBOARD_FILES => Self::ClipboardFiles(decode_manifest_len(ty, payload, &mut f)?),
            FILE_WANTED => {
                exactly(ty, payload, 8)?;
                Self::FileWanted {
                    set: f.u32()?,
                    index: f.u32()?,
                }
            }
            FILE_CONTENT => {
                exactly(ty, payload, 12)?;
                Self::FileContent {
                    index: f.u32()?,
                    len: f.u64()?,
                }
            }
            FILE_CHUNK => Self::FileChunk(decode_chunk(ty, payload, &mut f)?),
            FILE_ABANDONED => {
                exactly(ty, payload, 4)?;
                Self::FileAbandoned { index: f.u32()? }
            }
            FILE_CONSUMED => {
                exactly(ty, payload, 4)?;
                Self::FileConsumed(f.u32()?)
            }
            FILE_UNWANTED => {
                exactly(ty, payload, 4)?;
                Self::FileUnwanted { index: f.u32()? }
            }
            _ => return Err(ProtocolError::UnknownType { ty }),
        };
        f.end()?;
        Ok(message)
    }
}

/// A text run's length, if it is within the limit.
const fn text_len(len: u32) -> Result<u32, EncodeError> {
    if len > MAX_TEXT_LEN {
        Err(EncodeError::InvalidField)
    } else {
        Ok(len)
    }
}

/// An image run's length, if it is within the limit.
const fn image_len(len: u32) -> Result<u32, EncodeError> {
    if len > MAX_IMAGE_LEN {
        Err(EncodeError::InvalidField)
    } else {
        Ok(len)
    }
}

/// A manifest run's length, if it is within the bound.
const fn manifest_len(len: u32) -> Result<u32, EncodeError> {
    if len > MAX_MANIFEST_LEN {
        Err(EncodeError::InvalidField)
    } else {
        Ok(len)
    }
}

fn decode_manifest_len(ty: u16, payload: &[u8], f: &mut Fields<'_>) -> Result<u32, ProtocolError> {
    exactly(ty, payload, 4)?;
    let len = f.u32()?;
    if len > MAX_MANIFEST_LEN {
        return Err(ProtocolError::InvalidField { ty });
    }
    Ok(len)
}

/// A chunk's bytes, if there are 1 to [`frame::MAX_PAYLOAD`] of them.
fn chunk(bytes: &[u8]) -> Result<&[u8], EncodeError> {
    if bytes.is_empty() || bytes.len() > frame::MAX_PAYLOAD {
        Err(EncodeError::InvalidField)
    } else {
        Ok(bytes)
    }
}

fn decode_text_len(ty: u16, payload: &[u8], f: &mut Fields<'_>) -> Result<u32, ProtocolError> {
    exactly(ty, payload, 4)?;
    let len = f.u32()?;
    if len > MAX_TEXT_LEN {
        return Err(ProtocolError::InvalidField { ty });
    }
    Ok(len)
}

fn decode_image_len(ty: u16, payload: &[u8], f: &mut Fields<'_>) -> Result<u32, ProtocolError> {
    exactly(ty, payload, 4)?;
    let len = f.u32()?;
    if len > MAX_IMAGE_LEN {
        return Err(ProtocolError::InvalidField { ty });
    }
    Ok(len)
}

fn decode_chunk(ty: u16, payload: &[u8], f: &mut Fields<'_>) -> Result<Vec<u8>, ProtocolError> {
    if payload.is_empty() {
        return Err(ProtocolError::InvalidField { ty });
    }
    Ok(f.bytes(payload.len())?.to_vec())
}

fn decode_screen(f: &mut Fields<'_>) -> Result<Screen, ProtocolError> {
    let n = usize::from(f.u8()?);
    if n > MAX_MONITORS {
        return Err(ProtocolError::InvalidScreen);
    }
    let mut monitors = Vec::with_capacity(n);
    for _ in 0..n {
        let len = usize::from(f.u16()?);
        if len == 0 || len > MAX_ID_LEN {
            return Err(ProtocolError::InvalidScreen);
        }
        let id = core::str::from_utf8(f.bytes(len)?).map_err(|_| ProtocolError::InvalidScreen)?;
        let rect = Rect::new(f.i32()?, f.i32()?, f.i32()?, f.i32()?);
        let dpi = f.u32()?;
        if !plausible(&rect, dpi) {
            return Err(ProtocolError::InvalidScreen);
        }
        monitors.push(Monitor {
            id: MonitorId(id.to_owned()),
            rect,
            dpi,
        });
    }
    Ok(Screen::new(monitors))
}

// ---- service → agent ------------------------------------------------------

/// A message from the service to an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToAgent {
    SetCapture(CaptureMode),
    Inject(InputAction),
    Warp(Point),
    /// The service's clock reading, to be echoed in `Pong`.
    Ping(Millis),
}

impl ToAgent {
    pub const fn ty(&self) -> u16 {
        match self {
            Self::SetCapture(_) => SET_CAPTURE,
            Self::Inject(a) => match a {
                InputAction::Key { .. } => INJECT_KEY,
                InputAction::Button { .. } => INJECT_BUTTON,
                InputAction::Wheel { .. } => INJECT_WHEEL,
                InputAction::Motion { .. } => INJECT_MOTION,
            },
            Self::Warp(_) => WARP,
            Self::Ping(_) => PING,
        }
    }

    pub fn payload(&self) -> Vec<u8> {
        let mut p = Vec::new();
        match *self {
            Self::SetCapture(mode) => p.push(mode_code(mode)),
            Self::Inject(InputAction::Key { key, down }) => {
                p.extend_from_slice(&key.0.to_le_bytes());
                p.push(down_code(down));
            }
            Self::Inject(InputAction::Button { button, down }) => {
                p.push(button_code(button));
                p.push(down_code(down));
            }
            Self::Inject(InputAction::Wheel { dx, dy } | InputAction::Motion { dx, dy }) => {
                p.extend_from_slice(&dx.to_le_bytes());
                p.extend_from_slice(&dy.to_le_bytes());
            }
            Self::Warp(to) => {
                p.extend_from_slice(&to.x.to_le_bytes());
                p.extend_from_slice(&to.y.to_le_bytes());
            }
            Self::Ping(t) => p.extend_from_slice(&t.0.to_le_bytes()),
        }
        p
    }

    pub fn encode_frame(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        frame::encode(self.ty(), &self.payload(), out)
    }

    /// Decodes a message an agent received, after `Hello`.
    pub fn decode(ty: u16, payload: &[u8], version: u16) -> Result<Self, ProtocolError> {
        check(ty, Direction::ServiceToAgent, version)?;
        let mut f = Fields::new(ty, payload);
        let message = match ty {
            HELLO => return Err(ProtocolError::DuplicateHello),
            SET_CAPTURE => {
                exactly(ty, payload, 1)?;
                Self::SetCapture(mode_from(ty, f.u8()?)?)
            }
            INJECT_KEY => {
                exactly(ty, payload, 3)?;
                let key = Key(f.u16()?);
                Self::Inject(InputAction::Key {
                    key,
                    down: down_from(ty, f.u8()?)?,
                })
            }
            INJECT_BUTTON => {
                exactly(ty, payload, 2)?;
                let button = button_from(ty, f.u8()?)?;
                Self::Inject(InputAction::Button {
                    button,
                    down: down_from(ty, f.u8()?)?,
                })
            }
            INJECT_WHEEL => {
                exactly(ty, payload, 8)?;
                Self::Inject(InputAction::Wheel {
                    dx: f.i32()?,
                    dy: f.i32()?,
                })
            }
            INJECT_MOTION => {
                exactly(ty, payload, 8)?;
                Self::Inject(InputAction::Motion {
                    dx: f.i32()?,
                    dy: f.i32()?,
                })
            }
            WARP => {
                exactly(ty, payload, 8)?;
                Self::Warp(Point::new(f.i32()?, f.i32()?))
            }
            PING => {
                exactly(ty, payload, 8)?;
                Self::Ping(Millis(f.u64()?))
            }
            _ => return Err(ProtocolError::UnknownType { ty }),
        };
        f.end()?;
        Ok(message)
    }
}

/// A copy from the service to an agent. Kept apart from [`ToAgent`], which
/// stays `Copy` with fixed layouts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardToAgent {
    /// A run of text to set starts: its length in bytes of UTF-8.
    Set(u32),
    /// A run of an image to set starts: its length in bytes of PNG. Version 2.
    SetImage(u32),
    /// Bytes within the run; 1 to 65,536 of them.
    Chunk(Vec<u8>),
    /// A run of a file copy's manifest to offer for paste starts: its
    /// length. Version 3.
    SetFiles(u32),
    /// Serve file `index` of the file copy reported as number `copy`.
    /// Version 3.
    SendFile { copy: u32, index: u32 },
    /// The wanted file `index` starts: `len` bytes follow in
    /// `SetFileChunk`s. Version 3.
    SetFileContent { index: u32, len: u64 },
    /// Bytes of the file being set; 1 to 65,536 of them. Version 3.
    SetFileChunk(Vec<u8>),
    /// The file `index` being set will not complete, ending its run.
    /// Version 3.
    AbandonFile { index: u32 },
    /// The service passed on this many more bytes of the file being served.
    /// Version 3.
    FileAcked(u32),
    /// Stop serving file `index`. Version 3.
    StopFile { index: u32 },
}

impl ClipboardToAgent {
    pub const fn ty(&self) -> u16 {
        match self {
            Self::Set(_) => SET_CLIPBOARD,
            Self::SetImage(_) => SET_CLIPBOARD_IMAGE,
            Self::Chunk(_) => SET_CLIPBOARD_CHUNK,
            Self::SetFiles(_) => SET_CLIPBOARD_FILES,
            Self::SendFile { .. } => SEND_FILE,
            Self::SetFileContent { .. } => SET_FILE_CONTENT,
            Self::SetFileChunk(_) => SET_FILE_CHUNK,
            Self::AbandonFile { .. } => ABANDON_FILE,
            Self::FileAcked(_) => FILE_ACKED,
            Self::StopFile { .. } => STOP_FILE,
        }
    }

    pub fn payload(&self) -> Result<Vec<u8>, EncodeError> {
        Ok(match self {
            Self::Set(len) => text_len(*len)?.to_le_bytes().to_vec(),
            Self::SetImage(len) => image_len(*len)?.to_le_bytes().to_vec(),
            Self::Chunk(bytes) | Self::SetFileChunk(bytes) => chunk(bytes)?.to_vec(),
            Self::SetFiles(len) => manifest_len(*len)?.to_le_bytes().to_vec(),
            Self::SendFile { copy, index } => [copy.to_le_bytes(), index.to_le_bytes()].concat(),
            Self::SetFileContent { index, len } => {
                [&index.to_le_bytes()[..], &len.to_le_bytes()[..]].concat()
            }
            Self::AbandonFile { index } | Self::StopFile { index } => index.to_le_bytes().to_vec(),
            Self::FileAcked(n) => n.to_le_bytes().to_vec(),
        })
    }

    pub fn encode_frame(&self, out: &mut Vec<u8>) -> Result<(), EncodeError> {
        frame::encode(self.ty(), &self.payload()?, out)
    }

    /// Decodes a clipboard message an agent received, after `Hello`.
    pub fn decode(ty: u16, payload: &[u8], version: u16) -> Result<Self, ProtocolError> {
        check(ty, Direction::ServiceToAgent, version)?;
        let mut f = Fields::new(ty, payload);
        let message = match ty {
            SET_CLIPBOARD => Self::Set(decode_text_len(ty, payload, &mut f)?),
            SET_CLIPBOARD_IMAGE => Self::SetImage(decode_image_len(ty, payload, &mut f)?),
            SET_CLIPBOARD_CHUNK => Self::Chunk(decode_chunk(ty, payload, &mut f)?),
            SET_CLIPBOARD_FILES => Self::SetFiles(decode_manifest_len(ty, payload, &mut f)?),
            SEND_FILE => {
                exactly(ty, payload, 8)?;
                Self::SendFile {
                    copy: f.u32()?,
                    index: f.u32()?,
                }
            }
            SET_FILE_CONTENT => {
                exactly(ty, payload, 12)?;
                Self::SetFileContent {
                    index: f.u32()?,
                    len: f.u64()?,
                }
            }
            SET_FILE_CHUNK => Self::SetFileChunk(decode_chunk(ty, payload, &mut f)?),
            ABANDON_FILE => {
                exactly(ty, payload, 4)?;
                Self::AbandonFile { index: f.u32()? }
            }
            FILE_ACKED => {
                exactly(ty, payload, 4)?;
                Self::FileAcked(f.u32()?)
            }
            STOP_FILE => {
                exactly(ty, payload, 4)?;
                Self::StopFile { index: f.u32()? }
            }
            _ => return Err(ProtocolError::UnknownType { ty }),
        };
        f.end()?;
        Ok(message)
    }
}

/// Any message an agent receives after `Hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToAgentAny {
    Command(ToAgent),
    Clipboard(ClipboardToAgent),
}

impl ToAgentAny {
    pub fn decode(ty: u16, payload: &[u8], version: u16) -> Result<Self, ProtocolError> {
        match ty {
            SET_CLIPBOARD | SET_CLIPBOARD_IMAGE | SET_CLIPBOARD_CHUNK | SET_CLIPBOARD_FILES
            | SEND_FILE | SET_FILE_CONTENT | SET_FILE_CHUNK | ABANDON_FILE | FILE_ACKED
            | STOP_FILE => ClipboardToAgent::decode(ty, payload, version).map(Self::Clipboard),
            _ => ToAgent::decode(ty, payload, version).map(Self::Command),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(id: &str, w: i32, dpi: u32) -> Monitor {
        Monitor {
            id: MonitorId(id.into()),
            rect: Rect::new(-1920, 0, w, 1080),
            dpi,
        }
    }

    fn to_service() -> Vec<ToService> {
        let at = Millis(0x0102_0304_0506_0708);
        let input = |input| ToService::Input {
            at,
            input,
            origin: Origin::Injected,
        };
        vec![
            input(InputEvent::Key {
                key: Key(0xe4),
                down: true,
            }),
            input(InputEvent::Button {
                button: Button::X2,
                down: false,
            }),
            input(InputEvent::Wheel { dx: -120, dy: 240 }),
            input(InputEvent::Motion {
                dx: -3,
                dy: 7,
                cursor: Point::new(-5, 1079),
            }),
            ToService::Screen(Screen::new(vec![
                monitor("edid:DEL4123:ABC", 1920, 96),
                monitor("path:x", 2560, 144),
            ])),
            ToService::Screen(Screen::default()),
            ToService::Pong {
                service: Millis(1),
                agent: Millis(u64::MAX),
            },
        ]
    }

    fn to_agent() -> Vec<ToAgent> {
        vec![
            ToAgent::SetCapture(CaptureMode::WithholdMouse),
            ToAgent::Inject(InputAction::Key {
                key: Key(0x73),
                down: false,
            }),
            ToAgent::Inject(InputAction::Button {
                button: Button::Middle,
                down: true,
            }),
            ToAgent::Inject(InputAction::Wheel { dx: 1, dy: -1 }),
            ToAgent::Inject(InputAction::Motion { dx: 30, dy: -2 }),
            ToAgent::Warp(Point::new(1919, -540)),
            ToAgent::Ping(Millis(99)),
        ]
    }

    #[test]
    fn every_message_round_trips() {
        for m in to_service() {
            let p = m.payload().unwrap();
            assert_eq!(ToService::decode(m.ty(), &p, 1), Ok(m.clone()), "{m:?}");
        }
        for m in to_agent() {
            assert_eq!(ToAgent::decode(m.ty(), &m.payload(), 1), Ok(m), "{m:?}");
        }
    }

    #[test]
    fn fixed_lengths_match_the_design() {
        let lengths: Vec<usize> = to_service()
            .iter()
            .filter(|m| !matches!(m, ToService::Screen(_)))
            .map(|m| m.payload().unwrap().len())
            .collect();
        assert_eq!(lengths, [12, 11, 17, 25, 16]);
        let lengths: Vec<usize> = to_agent().iter().map(|m| m.payload().len()).collect();
        assert_eq!(lengths, [1, 3, 2, 8, 8, 8, 8]);
    }

    #[test]
    fn one_byte_short_or_long_is_rejected() {
        for m in to_service() {
            let p = m.payload().unwrap();
            let mut long = p.clone();
            long.push(0);
            assert!(ToService::decode(m.ty(), &long, 1).is_err(), "{m:?} long");
            if let Some((_, short)) = p.split_last() {
                assert!(ToService::decode(m.ty(), short, 1).is_err(), "{m:?} short");
            }
        }
        for m in to_agent() {
            let p = m.payload();
            let mut long = p.clone();
            long.push(0);
            assert!(ToAgent::decode(m.ty(), &long, 1).is_err(), "{m:?} long");
            let (_, short) = p.split_last().unwrap();
            assert!(ToAgent::decode(m.ty(), short, 1).is_err(), "{m:?} short");
        }
    }

    #[test]
    fn undefined_codes_are_rejected() {
        let key = |origin: u8, down: u8| {
            let mut p = 5u64.to_le_bytes().to_vec();
            p.extend([origin, 0x04, 0x00, down]);
            ToService::decode(KEY_EVENT, &p, 1)
        };
        assert_eq!(
            key(2, 1),
            Err(ProtocolError::InvalidCode {
                ty: KEY_EVENT,
                value: 2
            })
        );
        assert_eq!(
            key(0, 2),
            Err(ProtocolError::InvalidCode {
                ty: KEY_EVENT,
                value: 2
            })
        );
        assert_eq!(
            ToAgent::decode(INJECT_BUTTON, &[5, 1], 1),
            Err(ProtocolError::InvalidCode {
                ty: INJECT_BUTTON,
                value: 5
            })
        );
        assert_eq!(
            ToAgent::decode(SET_CAPTURE, &[3], 1),
            Err(ProtocolError::InvalidCode {
                ty: SET_CAPTURE,
                value: 3
            })
        );
    }

    #[test]
    fn wrong_direction_unknown_and_reserved() {
        let inject = ToAgent::Inject(InputAction::Key {
            key: Key(4),
            down: true,
        });
        assert_eq!(
            ToService::decode(INJECT_KEY, &inject.payload(), 1),
            Err(ProtocolError::WrongDirection { ty: INJECT_KEY })
        );
        assert_eq!(
            ToAgent::decode(PONG, &[0; 16], 1),
            Err(ProtocolError::WrongDirection { ty: PONG })
        );
        assert_eq!(
            ToService::decode(0x0042, &[], 1),
            Err(ProtocolError::UnknownType { ty: 0x0042 })
        );
        assert_eq!(
            ToAgent::decode(RESERVED, &[], 1),
            Err(ProtocolError::ReservedType)
        );
        assert_eq!(
            ToService::decode(HELLO, &[1, 0, 1, 0, 1], 1),
            Err(ProtocolError::DuplicateHello)
        );
    }

    #[test]
    fn clipboard_messages() {
        let services = [
            ToService::ClipboardText(0),
            ToService::ClipboardText(MAX_TEXT_LEN),
            ToService::ClipboardChunk(b"x".to_vec()),
            ToService::ClipboardChunk(vec![7; frame::MAX_PAYLOAD]),
            ToService::ClipboardNotShared(NotShared::Private),
            ToService::ClipboardNotShared(NotShared::TooLarge),
        ];
        for m in services {
            let p = m.payload().unwrap();
            assert_eq!(ToService::decode(m.ty(), &p, 1), Ok(m.clone()), "{m:?}");
        }
        let agents = [
            ClipboardToAgent::Set(5),
            ClipboardToAgent::Set(MAX_TEXT_LEN),
            ClipboardToAgent::Chunk(b"world".to_vec()),
        ];
        for m in agents {
            let p = m.payload().unwrap();
            assert_eq!(
                ToAgentAny::decode(m.ty(), &p, 1),
                Ok(ToAgentAny::Clipboard(m.clone()))
            );
        }

        // Fixed layouts, one byte short and one long.
        for (ty, len) in [(CLIPBOARD_TEXT, 4), (CLIPBOARD_NOT_SHARED, 1)] {
            for n in [len - 1, len + 1] {
                assert!(
                    ToService::decode(ty, &vec![0; n], 1).is_err(),
                    "{ty:#x} {n}"
                );
            }
        }
        for n in [3, 5] {
            assert!(ClipboardToAgent::decode(SET_CLIPBOARD, &vec![0; n], 1).is_err());
        }

        // Limits.
        let over = (MAX_TEXT_LEN + 1).to_le_bytes();
        assert_eq!(
            ToService::decode(CLIPBOARD_TEXT, &over, 1),
            Err(ProtocolError::InvalidField { ty: CLIPBOARD_TEXT })
        );
        assert_eq!(
            ClipboardToAgent::decode(SET_CLIPBOARD, &over, 1),
            Err(ProtocolError::InvalidField { ty: SET_CLIPBOARD })
        );
        assert_eq!(
            ToService::decode(CLIPBOARD_CHUNK, &[], 1),
            Err(ProtocolError::InvalidField {
                ty: CLIPBOARD_CHUNK
            })
        );
        assert_eq!(
            ClipboardToAgent::decode(SET_CLIPBOARD_CHUNK, &[], 1),
            Err(ProtocolError::InvalidField {
                ty: SET_CLIPBOARD_CHUNK
            })
        );
        assert_eq!(
            ToService::decode(CLIPBOARD_NOT_SHARED, &[3], 1),
            Err(ProtocolError::InvalidCode {
                ty: CLIPBOARD_NOT_SHARED,
                value: 3
            })
        );
        assert!(
            ToService::ClipboardText(MAX_TEXT_LEN + 1)
                .payload()
                .is_err()
        );
        assert!(ToService::ClipboardChunk(Vec::new()).payload().is_err());
        assert!(
            ToService::ClipboardChunk(vec![0; frame::MAX_PAYLOAD + 1])
                .payload()
                .is_err()
        );
        assert!(ClipboardToAgent::Chunk(Vec::new()).payload().is_err());

        // Each travels one way, on the agent link only.
        assert_eq!(
            ToService::decode(SET_CLIPBOARD, &[0; 4], 1),
            Err(ProtocolError::WrongDirection { ty: SET_CLIPBOARD })
        );
        assert_eq!(
            ToAgentAny::decode(CLIPBOARD_TEXT, &[0; 4], 1),
            Err(ProtocolError::WrongDirection { ty: CLIPBOARD_TEXT })
        );
        for ty in [
            CLIPBOARD_TEXT,
            CLIPBOARD_CHUNK,
            CLIPBOARD_NOT_SHARED,
            SET_CLIPBOARD,
            SET_CLIPBOARD_CHUNK,
        ] {
            assert_eq!(
                ui::UiCommand::decode(ty, &[0; 4], 1),
                Err(ProtocolError::WrongDirection { ty })
            );
            assert_eq!(
                check(ty, Direction::ServiceToUi, 1),
                Err(ProtocolError::WrongDirection { ty })
            );
        }
    }

    #[test]
    fn hello() {
        let agent = Hello::this_release(Role::SessionAgent);
        assert_eq!(agent.payload(), [0x01, 0x00, 0x03, 0x00, 0x01]);
        let mut longer = agent.payload().to_vec();
        longer.extend([9; 6]);
        assert_eq!(Hello::decode(&longer), Ok(agent));
        assert_eq!(
            Hello::decode(&[0, 0, 1, 0, 1]),
            Err(ProtocolError::InvalidVersionRange { min: 0, max: 1 })
        );
        assert_eq!(
            Hello::decode(&[3, 0, 2, 0, 1]),
            Err(ProtocolError::InvalidVersionRange { min: 3, max: 2 })
        );
        assert_eq!(
            Hello::decode(&[1, 0, 1, 0, 5]),
            Err(ProtocolError::InvalidCode {
                ty: HELLO,
                value: 5
            })
        );
        assert!(Hello::decode(&[1, 0, 1, 0]).is_err());

        let service = Hello::this_release(Role::Service);
        assert_eq!(service.agree(&agent), Some(3));
        // An agent from the previous release, as during an upgrade.
        let old_agent = Hello {
            min: 1,
            max: 1,
            role: Role::SessionAgent,
        };
        assert_eq!(service.agree(&old_agent), Some(1));
        // An agent that understands images but not files.
        let images_agent = Hello {
            min: 1,
            max: 2,
            role: Role::SessionAgent,
        };
        assert_eq!(service.agree(&images_agent), Some(2));
        let newer = Hello {
            min: 4,
            max: 5,
            role: Role::Service,
        };
        assert_eq!(newer.agree(&agent), None);
    }

    fn screen_payload(monitors: &[(&[u8], [i32; 4], u32)]) -> Vec<u8> {
        let mut p = vec![monitors.len() as u8];
        for (id, [x, y, w, h], dpi) in monitors {
            p.extend((id.len() as u16).to_le_bytes());
            p.extend(*id);
            for v in [x, y, w, h] {
                p.extend(v.to_le_bytes());
            }
            p.extend(dpi.to_le_bytes());
        }
        p
    }

    #[test]
    fn screen_limits() {
        let ok = [0, 0, 1920, 1080];
        let bad: Vec<Vec<u8>> = vec![
            {
                let mut p = vec![33u8];
                p.extend(screen_payload(&[(b"a", ok, 96)]).iter().skip(1));
                p
            },
            screen_payload(&[(b"", ok, 96)]),
            screen_payload(&[(&[b'a'; 257], ok, 96)]),
            screen_payload(&[(&[0xff, 0xfe], ok, 96)]),
            screen_payload(&[(b"a", [0, 0, 0, 1080], 96)]),
            screen_payload(&[(b"a", ok, 47)]),
            screen_payload(&[(b"a", ok, 961)]),
            screen_payload(&[(b"a", [i32::MAX, 0, 1, 1080], 96)]),
        ];
        for (i, p) in bad.iter().enumerate() {
            assert!(ToService::decode(SCREEN, p, 1).is_err(), "case {i}");
        }

        let ids: Vec<String> = (0..32).map(|i| format!("{i:0>256}")).collect();
        let big = Screen::new(
            ids.iter()
                .enumerate()
                .map(|(i, id)| Monitor {
                    id: MonitorId(id.clone()),
                    rect: Rect::new(i as i32 * 1920, 0, 1920, 1080),
                    dpi: 96,
                })
                .collect(),
        );
        let m = ToService::Screen(big);
        assert_eq!(ToService::decode(SCREEN, &m.payload().unwrap(), 1), Ok(m));

        let too_wide = ToService::Screen(Screen::new(vec![monitor("a", 0, 96)]));
        assert_eq!(too_wide.payload(), Err(EncodeError::InvalidScreen));
    }

    #[test]
    fn input_desktop_round_trip_and_limits() {
        for active in [false, true] {
            let m = ToService::InputDesktop(active);
            assert_eq!(m.payload().unwrap(), [u8::from(active)]);
            assert_eq!(
                ToService::decode(INPUT_DESKTOP, &[u8::from(active)], 1),
                Ok(m)
            );
        }
        assert_eq!(
            ToService::decode(INPUT_DESKTOP, &[2], 1),
            Err(ProtocolError::InvalidCode {
                ty: INPUT_DESKTOP,
                value: 2
            })
        );
        for bad in [&[][..], &[1, 0]] {
            assert!(matches!(
                ToService::decode(INPUT_DESKTOP, bad, 1),
                Err(ProtocolError::PayloadLength { .. })
            ));
        }
        // Agent → service only.
        assert_eq!(
            ToAgent::decode(INPUT_DESKTOP, &[1], 1),
            Err(ProtocolError::WrongDirection { ty: INPUT_DESKTOP })
        );
    }

    #[test]
    fn messages_per_agent_role() {
        let session = Role::SessionAgent;
        let winlogon = Role::WinlogonAgent;
        for ty in [CLIPBOARD_TEXT, CLIPBOARD_CHUNK, CLIPBOARD_NOT_SHARED] {
            assert_eq!(check_agent_role(ty, session), Ok(()));
            assert_eq!(
                check_agent_role(ty, winlogon),
                Err(ProtocolError::WrongRole { ty, role: 2 })
            );
        }
        assert_eq!(check_agent_role(INPUT_DESKTOP, winlogon), Ok(()));
        assert_eq!(
            check_agent_role(INPUT_DESKTOP, session),
            Err(ProtocolError::WrongRole {
                ty: INPUT_DESKTOP,
                role: 1
            })
        );
        for ty in [
            KEY_EVENT,
            BUTTON_EVENT,
            WHEEL_EVENT,
            MOTION_EVENT,
            SCREEN,
            PONG,
        ] {
            assert_eq!(check_agent_role(ty, session), Ok(()));
            assert_eq!(check_agent_role(ty, winlogon), Ok(()));
        }
    }

    #[test]
    fn registry_is_sorted_and_unique() {
        assert!(REGISTRY.windows(2).all(|w| w[0].ty < w[1].ty));
        assert!(
            REGISTRY
                .iter()
                .all(|i| i.ty != RESERVED && (1..=3).contains(&i.since))
        );
        let mut names: Vec<_> = REGISTRY.iter().map(|i| i.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), REGISTRY.len(), "a name is used twice");
    }

    #[test]
    fn version_3_adds_only_the_file_messages() {
        use Direction::{AgentToService as A, ServiceToAgent as S};
        let added: Vec<_> = REGISTRY
            .iter()
            .filter(|i| i.since == 3)
            .map(|i| (i.ty, i.name, i.direction))
            .collect();
        assert_eq!(
            added,
            [
                (0x0040, "ClipboardFiles", A),
                (0x0041, "FileWanted", A),
                (0x0042, "FileContent", A),
                (0x0043, "FileChunk", A),
                (0x0044, "FileAbandoned", A),
                (0x0045, "FileConsumed", A),
                (0x0046, "FileUnwanted", A),
                (0x0048, "SetClipboardFiles", S),
                (0x0049, "SendFile", S),
                (0x004a, "SetFileContent", S),
                (0x004b, "SetFileChunk", S),
                (0x004c, "AbandonFile", S),
                (0x004d, "FileAcked", S),
                (0x004e, "StopFile", S),
            ]
        );
        let to_service = [
            ToService::ClipboardFiles(0),
            ToService::ClipboardFiles(MAX_MANIFEST_LEN),
            ToService::FileWanted {
                set: 1,
                index: u32::MAX,
            },
            ToService::FileContent {
                index: 7,
                len: 5 << 32,
            },
            ToService::FileChunk(vec![1; frame::MAX_PAYLOAD]),
            ToService::FileAbandoned { index: 3 },
            ToService::FileConsumed(65_536),
            ToService::FileUnwanted { index: 2 },
        ];
        for m in to_service {
            let p = m.payload().unwrap();
            assert_eq!(ToService::decode(m.ty(), &p, 3), Ok(m.clone()));
            assert_eq!(
                ToService::decode(m.ty(), &p, 2),
                Err(ProtocolError::UnknownType { ty: m.ty() })
            );
            assert_eq!(check_agent_role(m.ty(), Role::SessionAgent), Ok(()));
            assert_eq!(
                check_agent_role(m.ty(), Role::WinlogonAgent),
                Err(ProtocolError::WrongRole {
                    ty: m.ty(),
                    role: 2
                })
            );
        }
        let to_agent = [
            ClipboardToAgent::SetFiles(12),
            ClipboardToAgent::SendFile { copy: 2, index: 0 },
            ClipboardToAgent::SetFileContent { index: 1, len: 0 },
            ClipboardToAgent::SetFileChunk(vec![2; 3]),
            ClipboardToAgent::AbandonFile { index: 1 },
            ClipboardToAgent::FileAcked(1),
            ClipboardToAgent::StopFile { index: 1 },
        ];
        for m in to_agent {
            let p = m.payload().unwrap();
            assert_eq!(
                ToAgentAny::decode(m.ty(), &p, 3),
                Ok(ToAgentAny::Clipboard(m.clone()))
            );
            assert_eq!(
                ToAgentAny::decode(m.ty(), &p, 2),
                Err(ProtocolError::UnknownType { ty: m.ty() })
            );
        }
        // Limits and layouts.
        assert!(
            ToService::ClipboardFiles(MAX_MANIFEST_LEN + 1)
                .payload()
                .is_err()
        );
        assert_eq!(
            ToService::decode(CLIPBOARD_FILES, &(MAX_MANIFEST_LEN + 1).to_le_bytes(), 3),
            Err(ProtocolError::InvalidField {
                ty: CLIPBOARD_FILES
            })
        );
        assert!(ToService::FileChunk(Vec::new()).payload().is_err());
        assert!(
            ClipboardToAgent::SetFileChunk(Vec::new())
                .payload()
                .is_err()
        );
        assert_eq!(
            ToService::decode(FILE_CONTENT, &[0; 11], 3),
            Err(ProtocolError::PayloadLength {
                ty: FILE_CONTENT,
                expected: 12,
                actual: 11
            })
        );
    }

    #[test]
    fn version_2_adds_only_the_image_runs() {
        let added: Vec<_> = REGISTRY
            .iter()
            .filter(|i| i.since == 2)
            .map(|i| (i.ty, i.name, i.direction))
            .collect();
        assert_eq!(
            added,
            [
                (0x001a, "ClipboardImage", Direction::AgentToService),
                (0x0029, "SetClipboardImage", Direction::ServiceToAgent),
            ]
        );
        // Undefined at version 1, whatever the payload.
        let len = 64u32.to_le_bytes();
        assert_eq!(
            ToService::decode(CLIPBOARD_IMAGE, &len, 1),
            Err(ProtocolError::UnknownType {
                ty: CLIPBOARD_IMAGE
            })
        );
        assert_eq!(
            ToAgentAny::decode(SET_CLIPBOARD_IMAGE, &len, 1),
            Err(ProtocolError::UnknownType {
                ty: SET_CLIPBOARD_IMAGE
            })
        );
        // Defined, and round-tripping, at version 2.
        for len in [8, MAX_IMAGE_LEN] {
            let m = ToService::ClipboardImage(len);
            assert_eq!(ToService::decode(m.ty(), &m.payload().unwrap(), 2), Ok(m));
            let c = ClipboardToAgent::SetImage(len);
            assert_eq!(
                ToAgentAny::decode(c.ty(), &c.payload().unwrap(), 2),
                Ok(ToAgentAny::Clipboard(c))
            );
        }
        // Over the limit, either way.
        let over = (MAX_IMAGE_LEN + 1).to_le_bytes();
        assert_eq!(
            ToService::decode(CLIPBOARD_IMAGE, &over, 2),
            Err(ProtocolError::InvalidField {
                ty: CLIPBOARD_IMAGE
            })
        );
        assert!(
            ToService::ClipboardImage(MAX_IMAGE_LEN + 1)
                .payload()
                .is_err()
        );
        assert!(
            ClipboardToAgent::SetImage(MAX_IMAGE_LEN + 1)
                .payload()
                .is_err()
        );
        // Only the session agent may send an image run.
        assert_eq!(
            check_agent_role(CLIPBOARD_IMAGE, Role::WinlogonAgent),
            Err(ProtocolError::WrongRole {
                ty: CLIPBOARD_IMAGE,
                role: 2
            })
        );
        assert_eq!(
            check_agent_role(CLIPBOARD_IMAGE, Role::SessionAgent),
            Ok(())
        );
    }
}
