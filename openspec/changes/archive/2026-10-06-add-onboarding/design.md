# Design

## Context

- **`apps/desktop` today:** a Tauri 2.12 shell with one 800×600 window and a React page that says "DeskHop".
  - The crate is `deskhop-desktop`, `#![forbid(unsafe_code)]`, and depends only on `tauri`.
  - CI builds the frontend (`npm ci`, `npm run build`) before `cargo test --workspace`, because `generate_context!` needs `dist/`.
- **The UI link (`add-ui-link`):**
  - `ipc::ui::UiEnd::connect(stream)` gives a `ServiceUiHandle` that sends `UiCommand`s without waiting, and a receiver of `UiEvent`s that ends when the link closes.
  - `UiEvent` is `Status` or a notice: `Paired`, `JoinFailed`, `PairingClosed`, `PairingNotOpened` or `Learned`.
  - The client writer silently skips a command it cannot encode, such as a code over 32 bytes or one with a space. The UI must therefore check a code before sending it.
- **The pipe:** `\\.\pipe\deskhop-ui`, today a constant in `bin/service/src/ui.rs`.
  - The registered service's ACL admits interactive users, and console mode admits only its own user.
  - The service closes a client that is not in the console session (or, in console mode, not in its own session).
- **The squatting item:** `add-ui-link` D6 left the UI's check that the server runs in session 0 to this change. The threat model lists it as open.
- **Codes:** `<locator>-<secret><check>`, for example `137-4829153`. The service parses them with `pairing::Code::parse`, which ignores whitespace, and answers a typo with `JoinFailed(MistypedCode)` before any attempt is spent.
- **Undo:** the engine offers it for 10 s after learning. Undo returns focus to this PC if it is on the learned peer.
- **Settled with the user:**
  - the session-0 check lives in a new `unsafe` crate, `win32-shell` (ADR 0007);
  - the app registers itself to start at logon, on by default, with a tray tick to turn it off;
  - earlier: any console user may use every command; a small Undo popup near the tray.

## Goals / Non-Goals

**Goals:**
- Every requirement in `specs/desktop-app`.
- The Rust side does all protocol work: connection, trust check, status and notices, code checks. React only renders view data and calls commands, so the parts that can go wrong are testable in Rust without a webview.
- The two manual checks deferred from `add-service` (two PCs, and Ctrl+Alt+End), run through the app and the registered service.

**Non-Goals:**
- Updater, installer, WebView2 bootstrapping (`add-installer`).
- Toast notifications, diagnostics, the arrangement view, machine rename.
- Multiple tray icon images. One icon; state goes in the tooltip.
- Localization. English strings live in one file, so translation can be added later.

## Decisions

### D1. ADR 0007: `win32-shell`

- **New crate `crates/win32-shell`.** `unsafe` is allowed, and every block carries `// SAFETY:`.
  - `pipe_server_session(&NamedPipeClient) -> io::Result<u32>` calls `GetNamedPipeServerSessionId`.
  - `own_session() -> io::Result<u32>` calls `ProcessIdToSessionId`.
  - Dependencies: `windows` (`Win32_Foundation`, `Win32_System_Pipes`, `Win32_System_RemoteDesktop`) and `tokio` (`net`).
- **`cargo archcheck`:** `win32-shell` joins `UNSAFE_CRATES` and `ENGINE_FORBIDDEN`, and `ONLY_LINKER` gains `("win32-shell", "deskhop-desktop")`.
- **Docs:**
  - AGENTS.md: rule 2 adds "only `apps/desktop/src-tauri` links `win32-shell`", rule 3 names four crates, and the repo layout lists it.
  - `docs/design-brief.md`: "confined to four crates".
- **`own_session` duplicates the one in `win32-session`.** That is two lines of `unsafe`. Sharing would need a crate both the service and the UI link, which defeats one-linker-per-crate.

*Alternatives (offered to the user):*
- Let the UI link `win32-session`. That puts launch, `SendSAS` and folder-ownership code in the user's process, and blurs the audit boundary.
- Skip the check. That leaves squatting open.

### D2. The service link (`src-tauri/src/link.rs`)

One task on Tauri's async runtime owns the connection:

```
loop {
  open \\.\pipe\deskhop-ui      NotFound / PIPE_BUSY -> NotRunning, wait 1 s
  server = pipe_server_session  trust(server, own, DEV)? else NotRunning(untrusted), drop, wait 1 s
  UiEnd::connect(pipe)          handshake error -> NotRunning, wait 1 s
  forward events -> Model       until the receiver ends
  Model::disconnected()         -> NotRunning, wait 1 s
}
```

- **The trust rule, `trust(server, own, dev) -> bool`:** `server == 0 || (dev && server == own)`, where `DEV = cfg!(debug_assertions)`. It is a pure function, unit-tested.
  - A release build is what the installer ships; it never takes the own-session branch.
- **The loop is generic over a connector,** an async function that returns a stream and a server session. Tests drive it with `tokio::io::duplex` and a real `ServiceUiEnd`, with no pipe and no Tauri.
- **`Model`** holds the connection state, the latest `Status` and the current `ServiceUiHandle`.
  - It turns each event into view data (D3) and hands it to a `Sink` trait.
  - Tauri's sink emits `deskhop://view` and `deskhop://notice` to the webviews, and updates the tray tooltip and the Undo popup.

*Alternative:* the link in TypeScript. A webview cannot open a named pipe, and the trust check needs Win32.

### D3. View data (`src-tauri/src/view.rs`)

Serde structs sent to React. All mapping is pure and unit-tested.

- **`View`:** `{ connected, status }`, where `status` is `null` while not connected, and otherwise `{ key, name, agentLinked, members, pairing, focus }`.
  - `members`: `{ key (64 hex), short (8 hex, shown only when two members share a name), name, up }`.
  - `pairing`: `closed`, or `{ open, code, leftMs }`.
  - `focus`: `local`, `on` or `controlledBy`, the last two with the member's key and name.
  - The agent-linked flag is mapped but not displayed in this change.
- **`Notice`** (tagged):
  - `paired { name }`;
  - `joinFailed { reason }`, `pairingClosed { reason }`, `pairingNotOpened`;
  - `learned { name, side, leftMs }`.
  - Reasons are stable string tags (`wrongCode`, `mistypedCode`, `notReady`, …), and React maps them to sentences.
  - Names in notices are resolved from the latest `Status` by key, falling back to the short key.
- **`tooltip(&View) -> String`:** Windows tooltips hold 127 characters; longer names are cut with "…". Examples:
  - "DeskHop: service isn't running";
  - "DeskHop: 2 of 3 PCs connected";
  - "DeskHop: controlling OFFICE";
  - "DeskHop: controlled by OFFICE".
- A command `view()` returns the current `View`, so a window that loads after an event still starts correct.

### D4. Commands and code checks

- **Tauri commands:**
  - `view`, and `route` (the page the shell asked the main window to open, taken once, so a window that loads late still opens *Add a PC*);
  - `open_pairing`, `close_pairing`, `join(code)`, `remove_peer(key)`, `undo`, `forget_layout`.
  - Start with Windows lives only in the tray menu, so it needs no command.
- **When not connected,** commands that need the service return `Err("notConnected")`. React also disables them.
- **`normalize_code(text) -> Result<String, InvalidCode>`:** it removes all whitespace, then requires 1 to `ipc::ui::MAX_CODE_LEN` bytes of printable ASCII (`0x21`–`0x7E`), the same rule the UI link applies.
  - Anything else returns `invalidCode`, and nothing is sent.
  - Typos and every other code rule stay with the service (`Code::parse`), so the UI never duplicates pairing logic.
- **`remove_peer` takes the key in hex.** It must decode to 32 bytes; anything else is an error and sends nothing.
- **Permissions:** `build.rs` declares the commands in an app manifest (`tauri_build::AppManifest`).
  - The `main` window's capability grants all of them.
  - The `undo` window's capability grants only `view` and `undo`.
  - No Tauri plugins with file, shell or HTTP access are added. The CSP stays `default-src 'self'`.

### D5. Windows and tray

- **`main` window.**
  - 440×600, created hidden at start.
  - Closing it is intercepted (`prevent_close`) and hides it.
  - It is shown by a tray click, *Open DeskHop*, *Add a PC* (which also emits `deskhop://navigate` `addPc`), a second launch, or first run.
- **`undo` window.**
  - 360×96, no decorations, always on top, skips the taskbar, created hidden.
  - It is made non-activating (`focusable(false)`, `WS_EX_NOACTIVATE` underneath) and shown without focus, so typing elsewhere keeps going (spec scenario).
- **Placing the popup.**
  - Take the tray icon's rectangle (`TrayIcon::rect`) and the work area of the monitor that holds it.
  - Put the popup in the work area's corner nearest the icon, 12 px in from the edges.
  - If Windows reports no rectangle, use the bottom-right corner of the primary monitor's work area.
  - Placement is a pure function of rectangles, unit-tested for a taskbar at the bottom, top, left and right.
- **Popup lifetime.**
  - On `learned`, Rust places and shows the popup and sends it the notice.
  - React counts down from `leftMs`, and renders nothing once it reaches zero.
  - Rust hides the window after `leftMs`, and at once on *Undo* (the `undo` command hides it), so the popup's window needs no window permissions.
  - A newer `learned` replaces the content and restarts the timer; a generation counter keeps a stale timer from hiding the newer popup.
- **Development builds only:** the tray item *Show test Undo popup* shows the popup for a made-up member, and its *Undo* sends nothing. It lets the no-focus behavior be checked without a second PC.
- **Tray.**
  - Tauri's `tray-icon` feature, with the app icon.
  - Menu: *Open DeskHop*, *Add a PC*, *Start with Windows* (a check item), a separator, *Quit*.
  - A left click opens `main`; the menu is on right click.
  - The tooltip is updated from D3's `tooltip` on every `View` change.
- **One frontend, two views.** React picks its view from the window label (`main` or `undo`).

### D6. First run

The first `View` with `connected: true` after the app starts decides: no members → show `main` on *Add a PC*; otherwise stay in the tray. Later reconnects don't reopen it. A PC that never pairs therefore opens *Add a PC* at each logon, which is the nudge zero configuration wants.

### D7. Start with Windows

- **`tauri-plugin-autostart`** writes the user's `Run` key (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `DeskHop`, the current exe path), using the registry without our own `unsafe`.
- **First launch:**
  - if the marker file `autostart-set` is missing from the app's config folder (`%APPDATA%\com.deskhop.desktop`), a release build enables autostart and writes the marker;
  - a development build writes nothing, so a `target\debug` path is never registered without the user ticking it.
- **The tick** reads `is_enabled()` at menu build and after each toggle. Toggling calls `enable`/`disable` and writes the marker, so a disabled state sticks.
- **Later,** when `add-installer` writes a machine-wide entry, it will set the marker for each user, or this logic will move. Recorded there.

### D8. One instance

`tauri-plugin-single-instance` is registered first. Its callback shows `main`. The plugin uses a per-user, per-session mutex, so the console user and an RDP user each get their own app. The RDP user's app is then refused by the service and shows "not running" — see Risks.

### D9. React

- **`src/bridge.ts`:** typed `invoke` and `listen` wrappers, the only file that imports `@tauri-apps/api`. Tests replace it with a fake.
- **`src/useDeskhop.ts`:** holds the latest `View`, the last notice, and the time a `View` arrived, which local countdowns use.
- **Views:**
  - `Main`: the header with this PC's name or "service isn't running", `Members`, *Add a PC*, and *Forget layout* with an inline confirm;
  - `AddPc`: *Show a code* (the code in large type, mm:ss left, *Cancel*, the close reason) and *Enter a code* (field, *Join*, joining state, result);
  - `UndoPopup`.
- **Confirmations are inline:** the button turns into "Remove OFFICE? *Remove* / *Cancel*". No dialog plugin.
- **`src/text.ts`:** every user-visible sentence, including each reason tag and side, as pure functions.
- **Styling:** plain CSS, Segoe UI, light and dark from `prefers-color-scheme`. No UI library.

### D10. Pipe name in `ipc`

`ipc::ui::PIPE_NAME = r"\\.\pipe\deskhop-ui"`, used by `bin/service` and the app. It changes no byte on the wire, so `IPC_VERSION` stays 1, and `docs/protocol.md` already names the pipe. The protocol-version rule covers wire changes; this is not one.

### D11. Tests

- **`win32-shell`** (normal `cargo test`): a test creates a pipe server with `tokio::net::windows::named_pipe::ServerOptions` and connects a client. `pipe_server_session(&client)` must equal `own_session()`. That is also the case a release build refuses, unless the test runs in session 0, which the test notes.
- **`deskhop-desktop` unit tests:**
  - `trust` (release and development);
  - `normalize_code` (spaces, 32 and 33 bytes, non-ASCII, empty);
  - key hex round trip;
  - short keys on name clashes;
  - every reason tag;
  - `tooltip` (each state, truncation);
  - popup placement (4 taskbar edges, no rectangle).
- **`deskhop-desktop` link tests** (tokio, duplex, a real `ServiceUiEnd`, a recording `Sink`):
  - `Status` becomes a `View`;
  - `join` reaches the service as `Join` with the normalized code;
  - an invalid code sends nothing;
  - the service closing gives `connected: false`, and a later connector success reconnects;
  - an untrusted server session sends nothing (the fake service sees no `Hello`);
  - a `Learned` notice resolves the member's name;
  - commands while disconnected return `notConnected`.
- **React (vitest, Testing Library, jsdom, fake bridge):**
  - not connected disables the commands;
  - *Show a code* shows the code and countdown, *Cancel* calls `close_pairing`, and a close reason is shown;
  - *Enter a code* shows joining, then the typo sentence, keeping the text;
  - *Remove* calls nothing until confirmed;
  - *Forget layout* confirm;
  - Undo popup counts down, *Undo* calls `undo`, and zero hides it.
- **Manual, on this PC:**
  - dev against console mode;
  - release against the registered service;
  - release refusing console mode;
  - single instance, autostart, the tick sticking;
  - the popup not stealing focus;
  - first run.
- **Manual, on two PCs (carried from add-service 8.4), with the registered service and the release app:**
  - pair through *Show a code* and *Enter a code*;
  - push across and see the Undo popup;
  - type and click there, cross back, take control back;
  - Ctrl+Alt+End on A while controlling B: B shows its Ctrl-Alt-Del screen;
  - Undo after a fresh learn;
  - restart both PCs and cross again with nothing re-paired or re-learned.

### D12. Threat model

The "UI → service" section closes its open item and adds:
- **Session-0 check:** the app trusts only a server in session 0. Any process in session 0 is a service, installed by an administrator, who is already outside the threat model. Development builds also accept their own session, and are never shipped.
- **The webview:** local content only (CSP `default-src 'self'`), no remote pages, only DeskHop's own commands, and the `undo` window limited to `view` and `undo`. The app runs as the user and holds no keys; the pairing code passes through it as it is shown or typed.
- **The Run entry:** it is in the user's own hive. Anything running as the user could change it, but such a process could already run anything as that user.

## Risks / Trade-offs

- **Undo needs the cursor back on this PC.** The learning crossing leaves the cursor on the peer, so clicking *Undo* means crossing back within 10 s. → Kept: the popup stays up for the full 10 s, and the reverse edge was learned, so crossing back is one push. A keyboard shortcut handled by the engine is a later change (listed out of scope).
- **`focusable(false)` might not stop activation on every Windows build.** → The manual check covers it. The fallback is showing with `SW_SHOWNOACTIVATE` through `win32-shell`, which would need a second function there.
- **Windows may not report a tray rectangle.** For example, when the icon sits in the overflow area. → The popup falls back to the primary work area's bottom-right corner.
- **An RDP user's app shows "isn't running".** The service only talks to the console session. → Accepted per the ui-link rule. A clearer message for that session can come with diagnostics.
- **First-run window at every logon until paired.** → Intended. The user can quit, and untick *Start with Windows*.
- **WebView2 missing on some Windows 10 PCs.** → The installer's bootstrapper (`add-installer`). The dev machines have it.
- **Plugin trust:** `tauri-plugin-single-instance` and `tauri-plugin-autostart` are official Tauri plugins with registry and mutex access only.

## Migration Plan

Nothing is deployed. The app gains a tray, and on a release build a per-user Run entry. Uninstalling before `add-installer` exists means unticking *Start with Windows*, or deleting the `DeskHop` value under the user's Run key.

## Open Questions

- The final tray icon artwork. The current `icon.ico` is used until there is a brand mark, and that changes no behavior.
