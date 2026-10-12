# ADR 0006 — `win32-session`: the service's Win32 calls in their own crate

**Status:** Proposed
**Date:** 2026-10-05
**Refines:** ADR 0002 (Process topology)

## Context

ADR 0002 has the SYSTEM service launch agents into user sessions with `WTSQueryUserToken` and `CreateProcessAsUser`. Doing that safely needs more Win32 than the `windows-service` crate offers:

- session and token queries;
- launching a process as the session user, on that session's desktop;
- named pipes created with a security descriptor, and the connected client's process ID;
- the owner and ACL of the service's data folder;
- `SendSAS`, and the policy that allows it.

All of these are `unsafe` calls. AGENTS.md rule 3 confines `unsafe` to `win32-input` and `win32-clipboard`, and rule 2 links those only into the agent, so the service never touches input. The service needs `unsafe` of its own, and it runs as SYSTEM, the process where an audit boundary matters most.

## Decision

- **A third `unsafe` crate, `crates/win32-session`.** It wraps each call above in a small safe function. Every `unsafe` block carries a `// SAFETY:` comment, as in the other two.
- **Linked only by `bin/service`.** `bin/service` keeps `#![forbid(unsafe_code)]`. `cargo archcheck` enforces both rules: `unsafe` is allowed in `win32-session`, and only `mouseflock-service` may depend on it.
- **No input.** `win32-session` has no hooks, no `SendInput` and no clipboard. The service still never touches input; `SendSAS` asks Windows to show its secure attention screen and injects nothing.
- **One launch function.** Launching into a session goes through one function in `bin/service`, which is the only caller of `win32-session`'s launch call.

## Alternatives considered

- **`unsafe` in `bin/service`.** That puts unaudited `unsafe` in the SYSTEM process next to the engine host. Rejected.
- **Put the calls in `win32-input`.** That would make the service link the input crate, breaking rule 2 and the reason for it. Rejected.
- **A shell-out to `sc.exe`, `icacls` or PowerShell.** That is fragile, slow, and harder to audit than a few typed calls. Rejected.

## Consequences

- `unsafe` lives in three crates. Each has a narrow job, and only the agent or only the service links each one.
- The service's security-relevant behavior (pipe ACLs, process checks, data folder ownership) sits in one small crate that reviewers can read end to end.
- Code that only SYSTEM can run (`WTSQueryUserToken`, `CreateProcessAsUser` with a user token, setting a folder's owner to SYSTEM) cannot be covered by ordinary `cargo test`. It is verified by hand through the registered service.
