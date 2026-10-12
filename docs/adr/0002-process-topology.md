# ADR 0002 — Service, two agents, optional UI

**Status:** Accepted
**Date:** 2026-09-27

## Context

Windows User Interface Privilege Isolation (UIPI) prevents a normal-integrity process from injecting input into elevated windows, UAC prompts, the lock screen, or Task Manager. Every lightweight software KVM breaks at exactly these moments, and users experience it as "it randomly stops working."

Separately, the app must keep working with no window open, at boot, on a headless machine, and while the UI is being upgraded. Input capture also needs the hooking thread to own a message pump and never stall.

## Decision

Every machine runs the same install with four processes:

| Process | Runs as | Responsibility |
|---|---|---|
| `mouseflock-service.exe` | Windows service, SYSTEM, session 0 | Transport, pairing keys, trust store, managed configuration, spawning agents |
| `mouseflock-agent.exe --session` | Interactive user session, medium integrity | Low-level hooks, Raw Input, `SendInput`, cursor clip/warp, clipboard |
| `mouseflock-agent.exe --winlogon` | Winlogon (secure) desktop, SYSTEM | Injection into UAC prompts and the lock screen |
| `MouseFlock.exe` | User, on demand | Tauri UI: onboarding, peers, layout, diagnostics |

The service spawns agents with `WTSQueryUserToken` + `CreateProcessAsUser`, reacting to session change notifications. All four processes communicate over named pipes using the versioned `ipc` crate. The service never touches input directly; the agents never touch the network.

The UI is a client of the service like any other. Closing it changes nothing.

## Alternatives considered

- **Single user-mode process** (Synergy classic). Simplest; fails at UAC and lock screen. Rejected.
- **`uiAccess=true` manifest** instead of a winlogon agent. Bypasses UIPI for higher-integrity windows but not the secure desktop; requires signed install under Program Files. Rejected as insufficient alone; may be layered later.
- **Engine inside the Tauri process.** Ties the daemon lifetime to a window. Rejected.

## Consequences

- Typing through a UAC prompt and unlocking a remote machine work. This is the reliability difference users notice.
- Three privilege levels means the threat model must cover the pipe boundary: agents and UI are less trusted than the service; the service validates everything it receives.
- Install is heavier: a service registration and a firewall rule, both done by the installer.
- Hook procedures must only enqueue events; Windows unhooks a slow hook silently.
- Auto-update must tolerate mixed versions across the four processes for the duration of an upgrade, which is why `ipc` is versioned from the first commit.
