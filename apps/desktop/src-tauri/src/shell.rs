//! The Tauri side (design D5–D8): the sink that feeds the windows and the
//! tray, the Undo popup, first run, and Start with Windows.

use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, Window, WindowEvent};
use tauri_plugin_autostart::ManagerExt as _;

use crate::checks::DEV;
use crate::link::{self, Link, Sink};
use crate::pipe;
use crate::place::{Rect, popup_position};
use crate::view::{Notice, SideTag, View, tooltip};

pub const MAIN: &str = "main";
pub const UNDO: &str = "undo";
const TRAY: &str = "tray";
/// The main window's *Add a PC* page.
const ADD_PC: &str = "addPc";
/// Logical pixels between the popup and the work area's edges.
const MARGIN: f64 = 12.0;
/// In the app's config folder once Start with Windows has been set, by the
/// first launch or by the user; the app never sets it again by itself.
const AUTOSTART_MARKER: &str = "autostart-set";

/// State the commands share with the sink.
pub struct Shell {
    app: AppHandle,
    /// Whether the first connected view has been seen (first run, D6).
    first_seen: AtomicBool,
    /// Bumped by every show or hide, so a stale timer hides nothing.
    popup: AtomicU64,
    /// The popup on screen is the development test one: its Undo sends nothing.
    test_popup: AtomicBool,
    /// A page the main window should open when it next asks.
    route: Mutex<Option<&'static str>>,
}

impl Shell {
    fn new(app: AppHandle) -> Arc<Self> {
        Arc::new(Self {
            app,
            first_seen: AtomicBool::new(false),
            popup: AtomicU64::new(0),
            test_popup: AtomicBool::new(false),
            route: Mutex::new(None),
        })
    }

    /// Shows the main window, on `route` if given.
    pub fn show_main(&self, route: Option<&'static str>) {
        if let Some(r) = route {
            *self.route.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
            let _ = self.app.emit_to(MAIN, "mouseflock://navigate", r);
        }
        if let Some(w) = self.app.get_webview_window(MAIN) {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
        }
    }

    /// The page the main window should open, once.
    pub fn take_route(&self) -> Option<&'static str> {
        self.route
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Places and shows the Undo popup without activating it, and hides it
    /// after `left_ms` unless something newer replaced it.
    fn show_popup(self: &Arc<Self>, notice: &Notice, left_ms: u32, test: bool) {
        let Some(w) = self.app.get_webview_window(UNDO) else {
            return;
        };
        self.test_popup.store(test, Ordering::SeqCst);
        let generation = self.popup.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some((x, y)) = self.popup_corner(&w) {
            let _ = w.set_position(PhysicalPosition::new(x, y));
        }
        let _ = self.app.emit_to(UNDO, "mouseflock://undo", notice);
        let _ = w.show();
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(u64::from(left_ms))).await;
            if me.popup.load(Ordering::SeqCst) == generation {
                me.hide_popup();
            }
        });
    }

    pub fn hide_popup(&self) {
        self.popup.fetch_add(1, Ordering::SeqCst);
        if let Some(w) = self.app.get_webview_window(UNDO) {
            let _ = w.hide();
        }
    }

    pub fn popup_is_test(&self) -> bool {
        self.test_popup.load(Ordering::SeqCst)
    }

    /// The popup's corner on the monitor holding the tray icon, or on the
    /// primary monitor when Windows reports no icon rectangle.
    fn popup_corner(&self, w: &tauri::WebviewWindow) -> Option<(i32, i32)> {
        let tray = self
            .app
            .tray_by_id(TRAY)
            .and_then(|t| t.rect().ok().flatten())
            .map(|r| {
                let p = r.position.to_physical::<i32>(1.0);
                let s = r.size.to_physical::<i32>(1.0);
                Rect {
                    x: p.x,
                    y: p.y,
                    width: s.width,
                    height: s.height,
                }
            });
        let monitor = tray
            .and_then(|t| {
                self.app
                    .monitor_from_point(f64::from(t.x), f64::from(t.y))
                    .ok()
                    .flatten()
            })
            .or_else(|| self.app.primary_monitor().ok().flatten())?;
        let area = monitor.work_area();
        let work = Rect {
            x: area.position.x,
            y: area.position.y,
            width: i32::try_from(area.size.width).ok()?,
            height: i32::try_from(area.size.height).ok()?,
        };
        let size = w.outer_size().ok()?;
        let size = (
            i32::try_from(size.width).ok()?,
            i32::try_from(size.height).ok()?,
        );
        // Rounded to whole pixels; at most a few hundred.
        #[allow(clippy::cast_possible_truncation)]
        let margin = (MARGIN * monitor.scale_factor()).round() as i32;
        Some(popup_position(tray, work, size, margin))
    }
}

/// Feeds the windows and the tray from the link.
struct ShellSink(Arc<Shell>);

impl Sink for ShellSink {
    fn view(&self, view: &View) {
        let shell = &self.0;
        let _ = shell.app.emit("mouseflock://view", view);
        if let Some(tray) = shell.app.tray_by_id(TRAY) {
            let _ = tray.set_tooltip(Some(tooltip(view)));
        }
        if let Some(status) = &view.status {
            let first = !shell.first_seen.swap(true, Ordering::SeqCst);
            if first && status.members.is_empty() {
                shell.show_main(Some(ADD_PC));
            }
        }
    }

    fn notice(&self, notice: &Notice) {
        let _ = self.0.app.emit("mouseflock://notice", notice);
        if let Notice::Learned { left_ms, .. } = notice {
            self.0.show_popup(notice, *left_ms, false);
        }
    }
}

/// Builds the shell: state, tray, Start with Windows, and the link task.
pub fn setup(app: &AppHandle) -> Result<(), Box<dyn Error>> {
    let shell = Shell::new(app.clone());
    app.manage(shell.clone());
    let link = Link::new(ShellSink(shell.clone()));
    app.manage(link.clone());
    tray(app, &shell)?;
    first_launch_autostart(app);
    let own = win32_shell::own_session()?;
    tauri::async_runtime::spawn(link::run(link, pipe::connect, own, DEV, link::RETRY));
    Ok(())
}

/// Closing a window hides it; only *Quit* ends the app.
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
    }
}

fn tray(app: &AppHandle, shell: &Arc<Shell>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open MouseFlock", true, None::<&str>)?;
    let add = MenuItem::with_id(app, "add", "Add a PC", true, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "Start with Windows",
        true,
        app.autolaunch().is_enabled().unwrap_or(false),
        None::<&str>,
    )?;
    let test = MenuItem::with_id(app, "test-undo", "Show test Undo popup", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = if DEV {
        Menu::with_items(app, &[&open, &add, &autostart, &test, &separator, &quit])?
    } else {
        Menu::with_items(app, &[&open, &add, &autostart, &separator, &quit])?
    };

    let on_menu = shell.clone();
    let on_click = shell.clone();
    let mut builder = TrayIconBuilder::with_id(TRAY)
        .tooltip("MouseFlock")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => on_menu.show_main(None),
            "add" => on_menu.show_main(Some(ADD_PC)),
            "autostart" => {
                let wanted = autostart.is_checked().unwrap_or(false);
                let _ = autostart.set_checked(set_autostart(app, wanted));
            }
            "test-undo" => on_menu.show_popup(&test_notice(), 10_000, true),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(move |_, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                on_click.show_main(None);
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// A made-up learned side for checking the popup without a second PC.
fn test_notice() -> Notice {
    Notice::Learned {
        key: "00".repeat(32),
        name: "TEST-PC".into(),
        side: SideTag::Right,
        left_ms: 10_000,
    }
}

/// On a release build's first launch for this user, registers the app to
/// start at logon (D7). Development builds never register by themselves.
fn first_launch_autostart(app: &AppHandle) {
    if DEV {
        return;
    }
    let Ok(dir) = app.path().app_config_dir() else {
        return;
    };
    let marker = dir.join(AUTOSTART_MARKER);
    if marker.exists() {
        return;
    }
    if app.autolaunch().enable().is_ok() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&marker, b"");
    }
}

/// Turns Start with Windows on or off for this user, and returns whether it
/// is now on. Either way the app will not turn it on again by itself.
fn set_autostart(app: &AppHandle, on: bool) -> bool {
    let manager = app.autolaunch();
    let _ = if on {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Ok(dir) = app.path().app_config_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(AUTOSTART_MARKER), b"");
    }
    manager.is_enabled().unwrap_or(false)
}
