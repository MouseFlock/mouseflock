# ADR 0007 — `win32-shell`: the UI's Win32 calls in their own crate

**Status:** Proposed
**Date:** 2026-10-05
**Refines:** ADR 0002 (Process topology), ADR 0006 (`win32-session`)

## Context

The UI (`MouseFlock.exe`) talks to the service over `\\.\pipe\mouseflock-ui`. A process that creates that name before the service could pose as the service: show a wrong pairing code, or collect a typed one. The service creates the name at boot, but the UI must also check what it connected to before trusting it. Only services run in session 0, so the UI checks that the pipe's server is in session 0 (`GetNamedPipeServerSessionId`).

That call is `unsafe`. The UI crate keeps `#![forbid(unsafe_code)]` (AGENTS.md rule 3). The calls the service needs live in `win32-session`, which only `bin/service` may link (ADR 0006); it also holds launching, `SendSAS` and data-folder ownership, which have no place in a process that runs as the user.

## Decision

- **A fourth `unsafe` crate, `crates/win32-shell`,** for the Win32 calls the UI needs. Today that is the pipe server's session and the UI's own session. Each is wrapped in a small safe function, and every `unsafe` block carries a `// SAFETY:` comment.
- **Linked only by `apps/desktop/src-tauri` (`mouseflock-desktop`).** That crate keeps `#![forbid(unsafe_code)]`. `cargo archcheck` enforces both rules, as for the other three crates.
- **No input, no privileges.** `win32-shell` has no hooks, no injection, no clipboard, and nothing that needs SYSTEM.
- **Small duplication is accepted.** The UI's own-session query repeats a two-line call in `win32-session`. Sharing it would need a crate linked by both the service and the UI, which undoes one linker per `unsafe` crate.

## Alternatives considered

- **Let the UI link `win32-session`.** That puts the service's launch, `SendSAS` and folder code in the user's process, and the audit boundary stops matching the process boundary. Rejected.
- **Skip the check.** That leaves pipe squatting open for any user process started before the service. Rejected.
- **`unsafe` in `mouseflock-desktop`.** That puts unaudited `unsafe` beside the webview bridge. Rejected.

## Consequences

- `unsafe` lives in four crates. Each has a narrow job, and exactly one binary links each.
- The UI trusts only a pipe server in session 0. Development builds also accept a server in their own session, for `mouseflock-service --console`; release builds never do.
- Further UI-side Win32 needs (for example, showing a window without activating it) go in `win32-shell` under the same rules.
