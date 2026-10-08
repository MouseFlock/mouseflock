# DeskHop

Software KVM for Windows. One keyboard and mouse across several PCs over the LAN. Zero configuration is the product; the engine is table stakes.

Read `docs/design-brief.md` once. Architectural decisions live in `docs/adr/` and are not relitigated in feature work. Behavior is specified in `openspec/`; use `/opsx:propose` before implementing anything non-trivial.

## Stack

- Rust, edition 2024, stable toolchain. Cargo workspace.
- Tauri 2 shell with React + TypeScript. The shell is thin: tray, autostart, updater, pipe client. No engine logic in `src-tauri`.
- Windows 10 22H2 and 11, x64 and ARM64. Nothing else.
- Key crates: `windows-rs` (Win32), `windows-service`, `quinn` (QUIC), `mdns-sd` (discovery), `rustls` (QUIC's TLS, pinned to identity keys), `spake2` (pairing PAKE), `tokio` (async, named pipes), `serde`.

## Process topology (ADR 0002)

Every machine runs the same install:

- `deskhop-service.exe` — Windows service, SYSTEM, session 0. Owns transport, pairing keys, managed configuration. Spawns agents via `WTSQueryUserToken` + `CreateProcessAsUser`.
- `deskhop-agent.exe --session --pipe <name>` — user desktop, as the logged-on user, with UI access (ADR 0008) so it reaches elevated windows. Low-level hooks, Raw Input, `SendInput`, clipboard.
- `deskhop-agent.exe --winlogon --pipe <name>` — secure desktop, as SYSTEM with privileges removed, whether or not anyone is logged on. UAC prompts, lock and logon screens. No clipboard.
- `DeskHop.exe` — Tauri UI. Optional. Nothing changes when it is closed.

Service, agents, and UI talk over named pipes using the `ipc` crate.

## Repo layout

```
crates/
  proto/            wire messages, framing, versioning — no I/O
  pairing/          identity keys, PAKE pairing, desk membership, trust store
  transport/        QUIC, mDNS discovery, datagrams for motion, streams for the rest
  model/            neutral input events, screen geometry, edge→peer layout graph
  engine/           state machine: focus owner, edge transitions, routing, layout learning
  ipc/              typed named-pipe protocol between service, agents, UI
  policy/           managed configuration: schema, signature check, registry source
  win32-input/      hooks, Raw Input, SendInput, cursor clip/warp, DPI     ← unsafe allowed
  win32-clipboard/  clipboard listener, text/DIB/HDROP, file transfer glue  ← unsafe allowed
  win32-session/    sessions, tokens, launching agents, pipe ACLs, SendSAS   ← unsafe allowed (ADR 0006)
  win32-shell/      the UI's Win32 calls: pipe server session check          ← unsafe allowed (ADR 0007)
bin/
  service/          deskhop-service.exe
  agent/            deskhop-agent.exe
apps/desktop/       Tauri 2 app (src-tauri + src)
installer/          WiX
docs/               design-brief.md, adr/, protocol.md, threat-model.md
openspec/           specs (source of truth) and changes
tools/              fuzzers, packet capture, event replay
```

## Dependency rules — enforced, not aspirational

1. `engine` depends on `model`, `proto`, `policy`. It never depends on `win32-*`, `transport`, or `ipc`. It must build and test on Linux.
2. Only `bin/agent` links `win32-input` and `win32-clipboard`. The service does not touch input. Only `bin/service` links `win32-session`. Only `apps/desktop/src-tauri` links `win32-shell`.
3. `#![forbid(unsafe_code)]` in every crate except `win32-input`, `win32-clipboard`, `win32-session` and `win32-shell`. Every `unsafe` block has a `// SAFETY:` comment.
4. `proto` and `ipc` are versioned from the first commit. Service, agents, and UI can be mid-upgrade at different versions.

## Win32 rules

- Hook procedures do nothing but enqueue. Windows silently unhooks a slow low-level hook.
- Per-monitor DPI awareness v2. Work in virtual-screen coordinates.
- Every spawn into a session or the secure desktop goes through one function in `bin/service` (`launch::agent`). No ad-hoc `CreateProcess`.
- The installer opens the firewall rule and registers the service. The user never does either.

## Commands

```
cargo build --workspace
cargo test --workspace                 # on Windows
cargo test -p proto -p model -p engine -p policy -p pairing -p ipc   # on Linux CI
cargo clippy --workspace -- -D warnings
cargo archcheck                        # dependency and unsafe rules
cargo build -p deskhop-agent; cargo run -p deskhop-service -- --console   # dev loop: pair, cross, type `help`
tools\dev\service.ps1 install|start|status|log|stop|uninstall             # registered service; elevated shell, run by the user
installer\build.ps1 -Arch x64|arm64                                        # the MSI and the setup program, in installer\out
installer\test.ps1 -Msi <msi> -UpgradeMsi <msi> -Setup <exe>               # install, upgrade, uninstall; elevated shell, run by the user
cargo check -p transport -p win32-input -p win32-session -p win32-shell -p deskhop-agent -p deskhop-service -p deskhop-desktop --target aarch64-pc-windows-msvc
cd apps/desktop && npm run tauri dev    # dev build trusts a console-mode service in this session
cd apps/desktop && npm test             # React tests
```

## Conventions

- Conventional commits, one line: `type(scope): subject`.
- A change to `proto` or `ipc` bumps the protocol version and updates `docs/protocol.md` in the same PR.
- New architectural decision → new ADR in `docs/adr/`, numbered, status Proposed until merged.
- Tests for `engine` are replay tests: a sequence of `model` events in, routing decisions out. Prefer these over mocking.
- Do not add dependencies to `proto`, `model`, or `engine` without a stated reason in the PR. These crates are the audit surface.
- AI-assisted contributions are welcome when tested and verified; say which agent and model in the PR.

## Do not

- Do not build a screen-layout grid, a server/client toggle, or an IP address field. If a task seems to need one, stop and ask. The one layout UI allowed is the optional, post-MVP arrangement view in ADR 0005.
- Do not put engine logic in `src-tauri` or in React.
- Do not add macOS or Linux code paths, `cfg` branches, or abstractions "for later".
- Do not call an LLM from anything in the input path.
