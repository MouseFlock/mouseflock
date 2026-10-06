// Declaring the app's commands makes each one a permission that a window's
// capability must grant (design D4).
const COMMANDS: &[&str] = &[
    "view",
    "route",
    "open_pairing",
    "close_pairing",
    "join",
    "remove_peer",
    "undo",
    "forget_layout",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}
