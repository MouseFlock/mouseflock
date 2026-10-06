# Tasks

## 1. ADR 0007 and `win32-shell`

- [x] 1.1 Write `docs/adr/0007-win32-shell.md` (status Proposed), giving the context, decision, alternatives and consequences in D1. Update AGENTS.md:
  - rule 2: only `apps/desktop/src-tauri` links `win32-shell`;
  - rule 3: four crates;
  - the repo layout: add `win32-shell/`, with "unsafe allowed (ADR 0007)";
  - the ARM64 command.

  Update `docs/design-brief.md` to say "confined to four crates". Verify by reading the ADR against ADR 0006's format, and by grepping the docs for "three crates" and "three `win32-*`", which should find none.
- [x] 1.2 Create `crates/win32-shell` with `pipe_server_session` and `own_session` (D1), each `unsafe` block carrying `// SAFETY:`. Add the test `server_session_matches_own`: a tokio pipe server and client in one process, where `pipe_server_session(&client) == own_session()`. Verify that `cargo test -p win32-shell` passes and that `cargo clippy -p win32-shell -- -D warnings` is clean.
- [x] 1.3 Extend `tools/archcheck`:
  - `UNSAFE_CRATES` and `ENGINE_FORBIDDEN` gain `win32-shell`;
  - `ONLY_LINKER` gains `("win32-shell", "deskhop-desktop")`;
  - update the module doc.

  Verify that `cargo archcheck` passes. Then temporarily add `win32-shell` to `bin/service`'s dependencies and confirm archcheck fails naming it, then revert.

## 2. Pipe name in `ipc`

- [x] 2.1 Add `ipc::ui::PIPE_NAME` (D10) and make `bin/service/src/ui.rs` use it in place of its own `UI_PIPE`. Verify that `cargo test -p ipc -p deskhop-service` passes and `docs/protocol.md` already names `\\.\pipe\deskhop-ui`, with `IPC_VERSION` unchanged.

## 3. View data and pure helpers in `deskhop-desktop`

- [x] 3.1 Add the dependencies `ipc`, `win32-shell`, `tokio` and `serde` (with `serde_json` for tests) to `apps/desktop/src-tauri`. Add `view.rs` with `View`, `Member`, `Pairing`, `Focus` and `Notice`, the reason tags, key hex and short keys (D3). Verify with unit tests:
  - a `Status` maps to the expected JSON;
  - every `JoinFailedReason` and `PairingClosedReason` has its tag;
  - short keys appear only on name clashes;
  - a `Learned` notice resolves the name from the latest status, and falls back to the short key for an unknown key.
- [x] 3.2 Add `trust`, `normalize_code` and `parse_key` (D2, D4). Verify with unit tests:
  - `trust`: session 0 is accepted in both builds, the own session only in development, and another session never;
  - `normalize_code`: `137 - 482 9153` becomes `137-4829153`, 32 bytes pass, 33 bytes, empty input, whitespace only and `é` are rejected;
  - `parse_key`: 64 hex characters pass, and 63 characters or non-hex are rejected.
- [x] 3.3 Add `tooltip(&View)` and `popup_position(tray_rect, work_area, size)` (D3, D5). Verify with unit tests:
  - the four tooltip states;
  - a 63-byte name is cut to fit 127 characters;
  - placement for a taskbar at the bottom, top, left and right, and the fallback with no tray rectangle.

## 4. The service link

- [x] 4.1 Add `link.rs`: the reconnect loop over a connector, `Model`, and the `Sink` trait (D2). Each command checks the connection and returns `notConnected` when down. Verify with tokio tests over `duplex` and a real `ServiceUiEnd` with a recording sink:
  - `status_becomes_view`;
  - `join_sends_normalized_code`;
  - `invalid_code_sends_nothing`;
  - `service_close_reports_not_connected_then_reconnects`;
  - `untrusted_server_gets_no_hello`;
  - `learned_names_the_member`;
  - `commands_while_down_fail`.
- [x] 4.2 Add the real connector: open `ipc::ui::PIPE_NAME` with `ClientOptions`, treat not-found and pipe-busy as not running, and check `win32_shell::pipe_server_session` with `trust` before `UiEnd::connect`. Verify with an `#[ignore]` desktop test against console mode. Run `cargo run -p deskhop-service -- --console` in another terminal, and the test then reaches a `View` with this PC's name. Verify too that the same test with `dev = false` reports not connected.

## 5. The Tauri shell

- [x] 5.1 Add the `tray-icon` feature and the plugins `tauri-plugin-single-instance` (registered first) and `tauri-plugin-autostart`. In `tauri.conf.json`, define the `main` window (440×600, hidden) and the `undo` window (360×96, no decorations, always on top, skips the taskbar, not focusable, hidden). Verify that `cargo build -p deskhop-desktop` succeeds after `npm run build`.
- [x] 5.2 Add the Tauri commands (D4). Declare them in the `build.rs` app manifest, grant all of them to `main` and only `view` and `undo` to `undo` in `capabilities/`, and keep the CSP. Verify by building. The `undo` window invoking `join` must be refused; check it from the dev tools console in the manual check (7.2).
- [x] 5.3 Add the Tauri `Sink`. It emits `deskhop://view` and `deskhop://notice`, sets the tray tooltip, and on `learned` places and shows the `undo` window with the backstop timer (D5). Add the tray menu and its left-click behavior, closing `main` to hide, and *Add a PC* navigation. In development builds only, add the tray item *Show test Undo popup*: it shows the popup with a made-up member, and its *Undo* sends nothing. Verify in `npm run tauri dev` against console mode:
  - the tooltip changes when a status changes;
  - closing the window keeps the tray icon;
  - *Quit* leaves the service running.
- [x] 5.4 Add first run (D6), single instance (D8) and Start with Windows (D7: the marker, development builds never registering by themselves, the tick). Verify by hand:
  - with an empty console-mode data folder, the window opens on *Add a PC*;
  - a second `DeskHop.exe` opens the existing window;
  - in a release build, the Run value appears on first launch, and after unticking and relaunching it stays gone.
- [x] 5.5 Update `docs/threat-model.md` (D12):
  - close the squatting open item and the related residual risk;
  - add the session-0 rule, the development-build exception, the webview's limits, and the Run entry.

  Verify by reading it against `specs/desktop-app` and the code paths in 4.2 and 5.2.

## 6. React UI

- [x] 6.1 Add `vitest`, `@testing-library/react` and `jsdom` as dev dependencies, and an `npm test` script. Write `src/bridge.ts` (the only importer of `@tauri-apps/api`), `src/useDeskhop.ts` and `src/text.ts` (D9). Verify with vitest unit tests of `text.ts`: every reason tag, every side, and the countdown format (`9:42`, `0:05`).
- [x] 6.2 Build `Main` and `Members`: the header, members with up or down and short keys, an inline-confirm *Remove*, an inline-confirm *Forget layout*, and the disconnected state. Verify with component tests against a fake bridge:
  - not connected disables the commands;
  - *Remove* calls nothing until confirmed, then `remove_peer` with the key;
  - *Forget layout* calls `forget_layout` only after confirming.
- [x] 6.3 Build `AddPc`. *Show a code* covers the code, the local countdown from `leftMs`, *Cancel*, the close reason, the no-network message, and a new code. *Enter a code* covers the field, *Join*, the joining state, and the result or reason, keeping the text. Verify with component tests:
  - showing a code and counting down;
  - *Cancel* calls `close_pairing`;
  - `tooManyFailures` offers a new code;
  - `mistypedCode` shows the typo sentence and keeps the text;
  - `paired` names the machine;
  - an `invalidCode` error from `join` shows "not a valid code".
- [x] 6.4 Build `UndoPopup`: the member and side, *Undo*, and the countdown, hiding at zero; a new notice restarts it. Add light and dark CSS. Verify with component tests: *Undo* calls `undo`, and fake timers reaching zero hide it.
- [x] 6.5 Add `npm test` to CI after `npm run build`. Add `-p win32-shell -p deskhop-desktop` to the ARM64 `cargo check`; if `deskhop-desktop` cannot check for ARM64, record why in the workflow and keep `win32-shell`. Verify that `npm test` passes locally and the ARM64 check runs locally.

## 7. Integration

- [x] 7.1 Run the full checks, and verify that each passes:
  - `cargo fmt --all --check`, `cargo archcheck`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`;
  - `npm run build` and `npm test` in `apps/desktop`;
  - the ARM64 check;
  - the ignored desktop tests in `win32-shell`, `deskhop-desktop` and `deskhop-service`.
- [x] 7.2 Manual check with the user on this PC. The user installs and starts the registered service (`tools/dev/service.ps1 install` and `start`, elevated) and builds a release app (`npm run tauri build -- --no-bundle`). Confirm:
  - the release app connects and shows this PC;
  - with the service stopped and `deskhop-service --console` running, the release app reports not connected, and the dev app connects;
  - *Show a code* shows a code and countdown, and *Cancel* closes it;
  - the Undo popup does not take focus from a window being typed in. In the dev app, use the debug-only tray item *Show test Undo popup* from 5.3; it is repeated in 7.3 with a real learn;
  - the `undo` window cannot call `join`;
  - one instance, first run, and the Start with Windows tick behave as in 5.4;
  - closing and quitting change nothing in the service log.
- [ ] 7.3 Manual check with the user on two PCs (carried from add-service 8.4), both with the registered service and the release app:
  - pair with *Show a code* on A and *Enter a code* on B;
  - push across, see the Undo popup on A, and choose *Undo* after crossing back once;
  - push again to learn, type and click on B, cross back, and take control back locally;
  - Ctrl+Alt+End on A while it controls B: B shows its Ctrl-Alt-Del screen;
  - restart both PCs and cross again with nothing re-paired or re-learned.

  If a second PC is not available, record that and leave this task open.

  Deferred by the user on 2026-10-06, until the rest of the product is complete. This task also carries add-service 8.4 and the end-to-end Ctrl+Alt+End check. On one PC, 7.2 covered everything except pairing, learning and crossing; the host tests over loopback cover the same path in-process.
