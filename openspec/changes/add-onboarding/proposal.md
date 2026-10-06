# Proposal

## Why

The service runs the engine, pairs and learns the layout, but only console mode can drive it. A user who installs DeskHop has no way to see the pairing code, type one in, see their machines, or take back a learned side. The design brief's promise ("install on two machines, type the code one shows into the other") needs the UI.

This is the second of two parts, as agreed with the user. `add-ui-link` gave the service a UI pipe and `ipc::UiEnd`. This change builds the Tauri app on that link.

## What Changes

- **`DeskHop.exe` becomes a working tray app** (`apps/desktop`). It stays thin: it shows what the service reports and passes the user's choices back as UI commands. No engine logic, no pairing logic.
  - **Tray icon** while running. A tooltip shows the state: not connected to the service, the number of machines up, or which machine is in control. Clicking opens the window. The menu has *Open DeskHop*, *Add a PC*, *Start with Windows* and *Quit*. Quit closes only the UI; the service carries on.
  - **One window.** It shows this PC's name, its machines (name, up or down, *Remove*), *Add a PC* and *Forget layout*. Closing the window hides it to the tray.
  - **Add a PC.** *Show a code*: opens pairing and shows the code with the time left until it closes. *Enter a code*: the user types the code shown on the other PC. Each failure reason gets a plain sentence (a typo, a wrong code, the other PC not ready, unreachable, and so on).
  - **First run.** When the app starts and this PC has no machines yet, the window opens on *Add a PC*. Otherwise the app starts in the tray.
  - **Undo popup near the tray**, as agreed with the user. When the service reports a learned side, a small popup names the machine and side with an *Undo* button and the seconds left. It never takes focus.
  - **One instance per user.** Starting it again shows the window.
  - **Start with Windows**, as agreed with the user. On first launch the app adds itself to the user's Run key, on by default. The tray's *Start with Windows* tick turns it off, and the app never turns it back on by itself.
  - **No service.** When the pipe is missing, the window and tooltip say DeskHop's service isn't running, and the app keeps retrying.
- **The UI trusts only the real service (closes the threat model's open item).**
  - Before sending `Hello`, the app checks that the pipe's server runs in session 0, where only services run (`GetNamedPipeServerSessionId`).
  - A development build also accepts a server in its own session, so it works with `deskhop-service --console`. A release build never does.
- **ADR 0007, `win32-shell`:** a fourth `unsafe` crate, linked only by `deskhop-desktop`, as agreed with the user.
  - It holds the UI's Win32 calls; today that is just the pipe server's session.
  - `cargo archcheck` enforces it. AGENTS.md and the design brief are updated (four `unsafe` crates).
- **The UI pipe's name moves into `ipc`** (`ipc::ui::PIPE_NAME`), so the service and the app share one constant. The wire format is unchanged, so there is no IPC version bump.
- **The checks deferred from `add-service` run here**, through the app and the registered service:
  - add-service 8.4 on two PCs: pair, learn with Undo, cross, type, take back, restart and cross again;
  - Ctrl+Alt+End end to end: the controlled PC shows its Ctrl-Alt-Del screen.

## Capabilities

### New Capabilities
- `desktop-app`: what `DeskHop.exe` does for the user. That covers how it finds and verifies the service, the tray, the window, the pairing flows, the Undo popup, start with Windows, and what it shows when the service is not there.

### Modified Capabilities
None. `ui-link` already specifies everything the service does for a UI. This change adds a client of it and moves a constant.

## Impact

- **Crates touched:**
  - **New `crates/win32-shell`:** `unsafe` allowed (ADR 0007). It contains `pipe_server_session`. Dependencies: `windows` and `tokio` (net).
  - **`apps/desktop/src-tauri` (`deskhop-desktop`):**
    - the service link, built on `ipc::ui::UiEnd`;
    - the tray, the windows, and the Tauri commands and events.
    - New dependencies: `ipc`, `win32-shell`, `tokio`, `serde`, `tauri-plugin-single-instance` and `tauri-plugin-autostart`, plus Tauri's `tray-icon` feature.
  - **`apps/desktop/src` (React):** the window and the Undo popup. New dev dependencies: `vitest`, `@testing-library/react` and `jsdom`.
  - **`ipc`:** `PIPE_NAME` only.
  - **`bin/service`:** uses `ipc::ui::PIPE_NAME`.
  - **`tools/archcheck`:** `win32-shell` added to its three lists.
  - **CI:** frontend tests, and an ARM64 check of `win32-shell` and `deskhop-desktop`.
  - **Docs:** ADR 0007, AGENTS.md, `docs/design-brief.md` and `docs/threat-model.md`.
  - **No change:** `engine`, `model`, `proto`, `policy`, `pairing`, `transport`, `bin/agent`, `win32-input`, `win32-clipboard`, `win32-session`.
- **Dependency rules hold:**
  - `engine` reaches none of `win32-*`, `transport` or `ipc`.
  - Only `deskhop-agent` links `win32-input` and `win32-clipboard`, only `deskhop-service` links `win32-session`, and only `deskhop-desktop` links `win32-shell`.
  - `deskhop-desktop` keeps `#![forbid(unsafe_code)]`.
  - `proto`, `model` and `engine` gain nothing.
- **No grid, no server/client toggle, no IP address field.**
  - Machines are shown by name, never by address.
  - Pairing is one code, typed on either PC, and the code's locator is never presented as an address.
  - Layout is learned; the only layout controls are Undo and Forget layout.
- **Out of scope:**
  - the updater, which needs signed releases and is `add-installer`'s concern;
  - the arrangement view (ADR 0005, post-MVP), diagnostics, and renaming machines;
  - Windows toast notifications;
  - a keyboard shortcut for Undo while the cursor is on the other PC, which is an engine change.
