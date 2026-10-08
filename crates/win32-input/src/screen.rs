//! The screen thread: monitors, their DPI and identities (design D7).

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Once;
use std::sync::mpsc::Sender;

use model::{Monitor, MonitorId, Rect, Screen};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DICS_FLAG_GLOBAL, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, DIREG_DEV, HDEVINFO,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA,
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, SetupDiOpenDevRegKey,
};
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GUID_DEVINTERFACE_MONITOR, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS,
    QueryDisplayConfig,
};
use windows::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, ERROR_SUCCESS, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{KEY_READ, RegCloseKey, RegQueryValueExW};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, DefWindowProcW, DestroyWindow, MSGFLT_ALLOW, RegisterClassW,
    SetTimer, WM_DEVICECHANGE, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_SETTINGCHANGE, WM_TIMER,
    WNDCLASSW,
};
use windows::core::{BOOL, PCWSTR, w};

use crate::capture::{Ready, hidden_window, message_loop, thread_id};
use crate::edid::{self, Edid, Target};
use crate::{Captured, desktop};

/// How often the screen is checked even without a display-change message.
const RECHECK_MS: u32 = 2000;
const SCREEN_TIMER: usize = 1;
const DESKTOP_TIMER: usize = 2;
const CLASS: PCWSTR = w!("DeskHopScreen");

/// What lies behind one monitor's identity, for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorDetail {
    /// The identity the monitor was given.
    pub id: MonitorId,
    /// The GDI device name, such as `\\.\DISPLAY1`.
    pub gdi_name: String,
    /// The display targets showing this monitor's area: one, or several
    /// when mirrored.
    pub targets: Vec<TargetDetail>,
}

/// One display target behind a monitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetDetail {
    /// The monitor device interface path.
    pub path: String,
    pub edid: Option<Edid>,
    /// The identity the rule gave this target.
    pub id: String,
}

/// Passes on a screen only when it differs from the last one passed.
#[derive(Debug, Default)]
pub(crate) struct ChangeFilter {
    last: Option<Screen>,
}

impl ChangeFilter {
    pub(crate) fn offer(&mut self, screen: &Screen) -> bool {
        if self.last.as_ref() == Some(screen) {
            return false;
        }
        self.last = Some(screen.clone());
        true
    }
}

thread_local! {
    /// Set by the window procedure when a display-change message arrives.
    static DIRTY: Cell<bool> = const { Cell::new(false) };
}

/// The screen thread. With `report_desktop`, it also reports whether its
/// desktop, the process's, is the input desktop: first right after the
/// first screen, then on each change.
pub(crate) fn thread(tx: Sender<Captured>, ready: Ready, report_desktop: bool) {
    let hwnd = match window() {
        Ok(h) => h,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    // When the agent runs at raised integrity (UI access), UIPI drops a
    // WM_SETTINGCHANGE that an unelevated process broadcasts, such as
    // Settings after a scaling change, so allow that one message through. It
    // is a no-op at medium integrity (add-elevated-windows D3).
    // SAFETY: the window belongs to this thread; a plain message id and flag,
    // no filter struct.
    unsafe {
        let _ = ChangeWindowMessageFilterEx(hwnd, WM_SETTINGCHANGE, MSGFLT_ALLOW, None);
    }
    // SAFETY: the window belongs to this thread; no callback is used.
    unsafe { SetTimer(Some(hwnd), SCREEN_TIMER, RECHECK_MS, None) };
    if report_desktop {
        // SAFETY: as above.
        unsafe { SetTimer(Some(hwnd), DESKTOP_TIMER, desktop::POLL_MS, None) };
    }
    let _ = ready.send(Ok(thread_id()));

    let mut edids = HashMap::new();
    let mut filter = ChangeFilter::default();
    let mut check = |edids: &mut HashMap<String, Option<Edid>>| {
        let (screen, details) = enumerate(edids);
        if filter.offer(&screen) {
            let _ = tx.send(Captured::Screen(screen, details));
        }
    };
    let mut desktops = desktop::Changes::default();
    let mut check_desktop = || {
        if let Some(active) = desktops.offer(desktop::own_is_input()) {
            let _ = tx.send(Captured::InputDesktop(active));
        }
    };
    check(&mut edids);
    if report_desktop {
        check_desktop();
    }
    message_loop(|msg| {
        let timer = (msg.message == WM_TIMER).then_some(msg.wParam.0);
        if timer == Some(SCREEN_TIMER) || DIRTY.take() {
            check(&mut edids);
        }
        if report_desktop && timer == Some(DESKTOP_TIMER) {
            check_desktop();
        }
    });
    // SAFETY: the window was created on this thread.
    let _ = unsafe { DestroyWindow(hwnd) };
}

/// A hidden window of our own class, whose procedure notes display changes:
/// those messages are sent, not posted, so the loop never sees them.
fn window() -> windows::core::Result<HWND> {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        // SAFETY: the class name is static and the procedure has the
        // WNDPROC signature. A failure shows up as CreateWindowExW failing.
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
    hidden_window(CLASS)
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if matches!(
        msg,
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE | WM_DEVICECHANGE
    ) {
        DIRTY.set(true);
    }
    // SAFETY: passes this window procedure's arguments on unchanged.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// One monitor as GDI reports it.
struct GdiMonitor {
    name: String,
    rect: Rect,
    dpi: u32,
}

/// The current screen, with identities from `QueryDisplayConfig` and EDIDs
/// (read once per device path and kept in `edids`).
fn enumerate(edids: &mut HashMap<String, Option<Edid>>) -> (Screen, Vec<MonitorDetail>) {
    let monitors = gdi_monitors();
    let paths = target_paths();

    if paths
        .iter()
        .any(|(_, p)| !edids.contains_key(&p.to_lowercase()))
    {
        let read = read_edids();
        for (_, p) in &paths {
            let key = p.to_lowercase();
            let edid = read.get(&key).cloned().flatten();
            edids.insert(key, edid);
        }
    }

    let targets: Vec<Target> = paths
        .iter()
        .map(|(_, p)| Target {
            path: p.clone(),
            edid: edids.get(&p.to_lowercase()).cloned().flatten(),
        })
        .collect();
    let ids = edid::target_ids(&targets);

    let mut out: Vec<(Monitor, MonitorDetail)> = monitors
        .into_iter()
        .map(|m| {
            let mine: Vec<TargetDetail> = paths
                .iter()
                .zip(&targets)
                .zip(&ids)
                .filter(|(((source, _), _), _)| source.eq_ignore_ascii_case(&m.name))
                .map(|((_, t), id)| TargetDetail {
                    path: t.path.clone(),
                    edid: t.edid.clone(),
                    id: id.clone(),
                })
                .collect();
            let id = if mine.is_empty() {
                MonitorId(format!("gdi:{}", m.name.to_lowercase()))
            } else {
                edid::monitor_id(mine.iter().map(|t| t.id.clone()).collect())
            };
            let detail = MonitorDetail {
                id: id.clone(),
                gdi_name: m.name,
                targets: mine,
            };
            let monitor = Monitor {
                id,
                rect: m.rect,
                dpi: m.dpi,
            };
            (monitor, detail)
        })
        .collect();
    out.sort_by(|a, b| (a.0.rect.y, a.0.rect.x, &a.0.id).cmp(&(b.0.rect.y, b.0.rect.x, &b.0.id)));
    let (monitors, details) = out.into_iter().unzip();
    (Screen::new(monitors), details)
}

fn wide(s: &[u16]) -> String {
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..end])
}

fn gdi_monitors() -> Vec<GdiMonitor> {
    unsafe extern "system" fn collect(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `&mut Vec<HMONITOR>` passed below, alive for
        // the whole enumeration.
        let list = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        list.push(m);
        BOOL(1)
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only pushes into `handles`, which outlives the
    // call.
    let _ = unsafe {
        EnumDisplayMonitors(None, None, Some(collect), LPARAM(&raw mut handles as isize))
    };
    handles
        .into_iter()
        .filter_map(|h| {
            let mut info = MONITORINFOEXW::default();
            info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
            let (mut dx, mut dy) = (0, 0);
            // SAFETY: `info` is a MONITORINFOEXW with cbSize set, which
            // GetMonitorInfoW may fill; the DPI outputs are valid u32s.
            unsafe {
                if !GetMonitorInfoW(h, (&raw mut info).cast::<MONITORINFO>()).as_bool() {
                    return None;
                }
                if GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_err() {
                    dx = 96;
                }
            }
            let r = info.monitorInfo.rcMonitor;
            Some(GdiMonitor {
                name: wide(&info.szDevice),
                rect: Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top),
                dpi: dx,
            })
        })
        .collect()
}

/// Every active path's `(GDI source name, monitor device path)`.
fn target_paths() -> Vec<(String, String)> {
    let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = Vec::new();
    let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = Vec::new();
    for _ in 0..3 {
        let (mut np, mut nm) = (0u32, 0u32);
        // SAFETY: both outputs are valid u32s.
        if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) }
            != ERROR_SUCCESS
        {
            return Vec::new();
        }
        paths.resize(np as usize, DISPLAYCONFIG_PATH_INFO::default());
        modes.resize(nm as usize, DISPLAYCONFIG_MODE_INFO::default());
        // SAFETY: the arrays hold `np` and `nm` elements, as passed.
        let r = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut np,
                paths.as_mut_ptr(),
                &mut nm,
                modes.as_mut_ptr(),
                None,
            )
        };
        if r == ERROR_INSUFFICIENT_BUFFER {
            continue;
        }
        if r != ERROR_SUCCESS {
            return Vec::new();
        }
        paths.truncate(np as usize);
        return paths
            .iter()
            .filter_map(|p| {
                let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                    header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                        size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                        adapterId: p.sourceInfo.adapterId,
                        id: p.sourceInfo.id,
                    },
                    ..Default::default()
                };
                let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                    header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                        r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                        size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                        adapterId: p.targetInfo.adapterId,
                        id: p.targetInfo.id,
                    },
                    ..Default::default()
                };
                // SAFETY: each packet starts with a header whose type and
                // size match the structure it is embedded in.
                let ok = unsafe {
                    DisplayConfigGetDeviceInfo(&mut source.header) == 0
                        && DisplayConfigGetDeviceInfo(&mut target.header) == 0
                };
                let path = wide(&target.monitorDevicePath);
                (ok && !path.is_empty()).then(|| (wide(&source.viewGdiDeviceName), path))
            })
            .collect();
    }
    Vec::new()
}

/// Every present monitor's EDID, keyed by lowercased device interface path.
fn read_edids() -> HashMap<String, Option<Edid>> {
    let mut out = HashMap::new();
    // SAFETY: a static GUID and flags; the handle is destroyed below.
    let Ok(set) = (unsafe {
        SetupDiGetClassDevsW(
            Some(&GUID_DEVINTERFACE_MONITOR),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    }) else {
        return out;
    };
    for index in 0.. {
        let mut iface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        // SAFETY: `iface` has cbSize set; `set` is a valid device info set.
        if unsafe {
            SetupDiEnumDeviceInterfaces(set, None, &GUID_DEVINTERFACE_MONITOR, index, &mut iface)
        }
        .is_err()
        {
            break;
        }
        if let Some((path, edid)) = interface_edid(set, &iface) {
            out.insert(path.to_lowercase(), edid);
        }
    }
    // SAFETY: `set` came from SetupDiGetClassDevsW and is not used again.
    let _ = unsafe { SetupDiDestroyDeviceInfoList(set) };
    out
}

/// One monitor interface's path and EDID.
fn interface_edid(
    set: HDEVINFO,
    iface: &SP_DEVICE_INTERFACE_DATA,
) -> Option<(String, Option<Edid>)> {
    let mut needed = 0u32;
    // SAFETY: a size query with no output buffer.
    let _ =
        unsafe { SetupDiGetDeviceInterfaceDetailW(set, iface, None, 0, Some(&mut needed), None) };
    if (needed as usize) < size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() {
        return None;
    }
    // u64 storage keeps the structure suitably aligned.
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    let detail = buf.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    let mut devinfo = SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    };
    // SAFETY: `buf` holds `needed` bytes, aligned for the structure, whose
    // cbSize is set as the API requires; `devinfo` has cbSize set.
    let path = unsafe {
        (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        SetupDiGetDeviceInterfaceDetailW(
            set,
            iface,
            Some(detail),
            needed,
            None,
            Some(&mut devinfo),
        )
        .ok()?;
        let start = (&raw const (*detail).DevicePath).cast::<u16>();
        let max = (needed as usize
            - std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath))
            / 2;
        wide(std::slice::from_raw_parts(start, max))
    };

    // SAFETY: `devinfo` was filled in above; the key is closed below.
    let key = unsafe {
        SetupDiOpenDevRegKey(set, &devinfo, DICS_FLAG_GLOBAL.0, 0, DIREG_DEV, KEY_READ.0)
    };
    let Ok(key) = key else {
        return Some((path, None));
    };
    let mut data = vec![0u8; 1024];
    let mut r = ERROR_MORE_DATA;
    let mut len = 0u32;
    // Retry once if the value outgrew the buffer.
    for _ in 0..2 {
        len = data.len() as u32;
        // SAFETY: `data` holds `len` writable bytes; the value name is
        // static.
        r = unsafe {
            RegQueryValueExW(
                key,
                w!("EDID"),
                None,
                None,
                Some(data.as_mut_ptr()),
                Some(&mut len),
            )
        };
        if r != ERROR_MORE_DATA {
            break;
        }
        data.resize(len as usize, 0);
    }
    // SAFETY: `key` was opened above and is not used again.
    let _ = unsafe { RegCloseKey(key) };
    let edid = (r == ERROR_SUCCESS)
        .then(|| data.get(..len as usize).and_then(edid::parse))
        .flatten();
    Some((path, edid))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(id: &str, dpi: u32, w: i32) -> Screen {
        Screen::new(vec![Monitor {
            id: MonitorId(id.into()),
            rect: Rect::new(0, 0, w, 1080),
            dpi,
        }])
    }

    #[test]
    fn change_filter() {
        let mut f = ChangeFilter::default();
        assert!(f.offer(&screen("a", 96, 1920)));
        assert!(!f.offer(&screen("a", 96, 1920)));
        assert!(f.offer(&screen("a", 144, 1920)));
        assert!(f.offer(&screen("a", 144, 2560)));
        assert!(f.offer(&screen("b", 144, 2560)));
        assert!(!f.offer(&screen("b", 144, 2560)));
    }
}
