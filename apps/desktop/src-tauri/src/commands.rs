//! The commands React may call (design D4). Each window's capability says
//! which: the Undo popup gets only `view` and `undo`.

use std::sync::Arc;

use ipc::ui::UiCommand;
use tauri::State;

use crate::link::{CommandError, Link};
use crate::shell::Shell;
use crate::view::View;

type Done = Result<(), CommandError>;

#[tauri::command]
pub fn view(link: State<'_, Arc<Link>>) -> View {
    link.view()
}

/// The page the main window should open, if the shell asked for one.
#[tauri::command]
pub fn route(shell: State<'_, Arc<Shell>>) -> Option<&'static str> {
    shell.take_route()
}

#[tauri::command]
pub fn open_pairing(link: State<'_, Arc<Link>>) -> Done {
    link.send(UiCommand::OpenPairing)
}

#[tauri::command]
pub fn close_pairing(link: State<'_, Arc<Link>>) -> Done {
    link.send(UiCommand::ClosePairing)
}

#[tauri::command]
pub fn join(link: State<'_, Arc<Link>>, code: String) -> Done {
    link.join(&code)
}

#[tauri::command]
pub fn remove_peer(link: State<'_, Arc<Link>>, key: String) -> Done {
    link.remove(&key)
}

/// Hides the popup, and asks the service to undo unless the popup was the
/// development test one.
#[tauri::command]
pub fn undo(link: State<'_, Arc<Link>>, shell: State<'_, Arc<Shell>>) -> Done {
    let test = shell.popup_is_test();
    shell.hide_popup();
    if test {
        return Ok(());
    }
    link.send(UiCommand::Undo)
}

#[tauri::command]
pub fn forget_layout(link: State<'_, Arc<Link>>) -> Done {
    link.send(UiCommand::ForgetLayout)
}
