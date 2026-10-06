# Proposal

## Why

The registered service runs the engine and launches the agent, but nothing outside console mode can drive it: there is no way to pair, see the peers, or take back a learned side. ADR 0002 gives the UI its own named pipe to the service, speaking the versioned `ipc` protocol. Today `ipc` defines the UI role and nothing else, and the service opens no UI pipe.

This change defines that link and serves it from the service. It is the first of two parts, as agreed with the user:
- `add-ui-link` (this change): the UI messages and the service's UI pipe, tested headless with a Rust client.
- `add-onboarding`: the Tauri app on top of it (tray, pairing screens, peers, the Undo popup, autostart).

## What Changes

- **UI messages in `ipc` (IPC version 1, unreleased, so no version bump).**
  - UI → service: `OpenPairing`, `ClosePairing`, `Join` (the code as typed), `RemovePeer`, `Undo`, `ForgetLayout`.
  - Service → UI:
    - `Status`: a snapshot of this machine (name, key), the members (name, key, up or down), pairing (closed, or open with its code and time left), focus (local, on a peer, or controlled by one), and whether the agent is linked;
    - notices for things that happen once: `Paired`, `JoinFailed` (with a reason, including a mistyped code), `PairingClosed` (with a reason), `PairingNotOpened`, and `Learned` (peer, side, and time left to Undo).
  - Types `0x0030`–`0x003F`. Each carries its direction, and a UI message on an agent link, or an agent message on a UI link, is a protocol error.
  - Fixed or bounded layouts: at most 256 members, names 1–63 bytes, codes at most 32 ASCII bytes. Decoding is under the same no-panic rules as the rest of `ipc`, with a never-panic test and the fuzz target extended.
  - `ipc` gains the UI link's async ends, as for the agent link: the service end pushes `Status` and notices and never waits on the UI, and a UI that stops reading is cut off.
- **The service serves the UI pipe.**
  - `\\.\pipe\deskhop-ui`, created by the service at start as the first instance. Up to 8 UIs at once.
  - **Who may connect:** any user at the console, as agreed with the user. The pipe's ACL admits SYSTEM and interactive users. The service accepts a client only if its session is the active console session. A UI in an RDP or disconnected session is closed. Any connected UI may pair, join, remove, undo and forget the layout; managed configuration may narrow this later.
  - On connect a UI gets `Status` at once, and again whenever anything in it changes, at most every 100 ms. Notices go to every connected UI.
  - Console mode serves the same pipe when it can create it, for the current user only, so `add-onboarding` can be developed without the registered service. If the name is taken, it logs that and runs without a UI pipe.
- **Docs.**
  - `docs/protocol.md`: the UI link: types, layouts, codes and behavior.
  - `docs/threat-model.md`: the UI pipe boundary: who can connect, what they can do, and the pipe-squatting risk with its mitigation (the service creates the name at boot, as the first instance; the UI-side check that the server runs in session 0 comes with `add-onboarding`).

## Capabilities

### New Capabilities
- `ui-link`: what the UI and the service exchange over the UI pipe. It covers who may connect, the status snapshot and when it is sent, the commands and their effects, the notices, and the limits that keep a slow or hostile UI from blocking the service.

### Modified Capabilities
None. `ipc-protocol` already defines the UI role, the registry with directions and strict decoding; this change adds types under those rules. Console commands in `service-host` stay as they are.

## Impact

- **Crates touched:**
  - **`ipc`:** UI message types, layouts and codes; `UiEnd` (client) and `ServiceUiEnd` (service). No new dependencies.
  - **`win32-session`:** the UI pipe (ACL for SYSTEM and interactive users, unlimited instances up to the service's cap) and the connected client's session ID (`GetNamedPipeClientSessionId`).
  - **`bin/service`:** the UI server task, and the host's status and notices. Console printing keeps its current form.
  - **`tools/fuzz`:** the `decode_ipc` target covers the UI direction.
  - **No change:** `engine`, `transport`, `pairing`, `proto`, `model`, `bin/agent`, `apps/desktop`.
- **Dependency rules hold:**
  - `engine` reaches no `ipc`.
  - Only the service links `win32-session`.
  - `ipc` stays `#![forbid(unsafe_code)]`.
  - `proto`, `model` and `engine` gain nothing.
- **No grid, no server/client toggle, no IP address field.** Peers are named by machine name and key, never by address. Pairing uses the code alone.
- **Later:** `add-onboarding` builds the Tauri app on this link, carries the deferred two-PC check from `add-service`, and checks Ctrl+Alt+End end to end once the registered service can pair.
