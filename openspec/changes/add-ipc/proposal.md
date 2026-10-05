# Proposal

## Why

The agent captures input and the transport carries engine messages between machines, but nothing connects the agent to the process that will run the engine. ADR 0002 puts the engine and network in a SYSTEM service and input in a per-session agent, with a versioned `ipc` protocol over named pipes between them. Today `ipc` is a version constant and `deskhop-agent --session` exits at once.

This change defines the agent link and makes the agent speak it. It is the first of two parts:
- `add-ipc` (this change): the link between agent and service.
- `add-service`: the Windows service itself, which creates the pipe, launches the agent, and runs the engine and node. The cursor first crosses between PCs at the end of that change.

## What Changes

- **IPC envelope.**
  - Every message on a pipe is one frame: a 4-byte little-endian payload length, a 2-byte message type, and the payload, at most 65,536 bytes.
  - Each side sends `Hello` first, with its lowest and highest IPC version and its role (session agent, winlogon agent or UI), and both use the highest common version.
  - Message types are registered with the version that introduced them. Unknown types, wrong lengths and undefined codes are protocol errors that end the connection.
  - Decoding never panics and never allocates from a claimed length, as in `proto`.
- **The agent link (IPC version 1).**
  - Agent to service:
    - `Input`: time, origin, and a key, button, wheel or motion event with the cursor position;
    - `Screen`: the monitors, each with identity, rectangle and DPI;
    - `Pong`.
  - Service to agent: `SetCapture`, `Inject`, `Warp` and `Ping`.
  - These carry the existing `model` types; `ipc` defines their byte layouts.
- **Clock.**
  - The agent stamps events with its monotonic clock. The service sends `Ping` with its own time, the agent answers `Pong` with both times, and the service keeps the offset from the exchange with the shortest round trip.
  - The service converts agent times into its own clock before the engine sees them.
  - This answers the clock question `add-input-capture` left open.
- **Safety on disconnect.** When the link closes for any reason (the service stops, the pipe breaks, or a protocol error), the agent returns capture to pass-all at once and exits. Input is never left withheld without a service.
- **`deskhop-agent --session --pipe <name>`.** The agent connects to the named pipe the service gives it, sends `Hello`, starts capture, forwards every captured event, and carries out every command. `--winlogon` keeps its stub; the winlogon agent is a later change.
- **Service end, without the service.** `ipc` provides both ends of the link as async connections over any byte stream. The service end validates everything the agent sends, including screen limits and finite values. `add-service` runs it behind a real pipe; this change tests it against a real agent over a test-created named pipe.
- **Docs.**
  - `docs/protocol.md` fills in the IPC section: frames, `Hello`, roles, every agent-link layout, and the clock exchange.
  - `docs/threat-model.md` covers the agent → service boundary: what the service accepts and why a stalled or hostile agent cannot block it.
  - `IPC_VERSION` stays 1, because it has never shipped.

## Capabilities

### New Capabilities
- `ipc-protocol`: the envelope of the named-pipe protocol between service, agents and UI. It covers frames, size limit, `Hello` with roles and version negotiation, the type registry, strict decoding, and closing on errors.
- `agent-link`: what the session agent and the service exchange. It covers captured events and screen reports up, capture mode, injection and warp down, the clock exchange, and the agent's behavior when the link closes.

### Modified Capabilities
None. `input-capture` already requires that nothing stays withheld after capture ends; this change makes the end of the link end capture.

## Impact

- **Crates:**
  - **`ipc`:**
    - The codec (sans-I/O, under the same no-panic lints as `proto`), the message types, and async `AgentEnd`/`ServiceEnd` connections.
    - Depends on `model` (types carried) and `tokio` (`io-util` for the async ends).
    - A never-panic test and a fuzz target.
  - **`bin/agent`:**
    - `--session --pipe <name>`: a tokio runtime with a named-pipe client (`tokio::net::windows::named_pipe::ClientOptions`, safe), bridging `win32-input` and `ipc`.
    - It stays `#![forbid(unsafe_code)]` and still never touches the network.
  - **`win32-input`:** no change. Its `now()` remains the agent's clock.
- **Dependency rules hold:**
  - `engine` reaches no `ipc`.
  - Only `deskhop-agent` links `win32-input`.
  - `ipc` and the agent forbid `unsafe`.
  - `model` gains no dependency.
  - No `cfg` branches for other operating systems: `ipc`'s codec has no Windows calls, and the agent is Windows-only as before.
- **No grid, no server/client toggle, no IP address field.**
- **Later changes:**
  - `add-service`, with ADR 0006: a new `win32-session` crate, `unsafe` allowed and linked only by `bin/service`. The service creates each agent's pipe under a random name with an ACL for SYSTEM and that session's user, launches the agent through the one spawn function, and runs the engine and node on the `ServiceEnd`. It also adds `deskhop-service --console` and a dev script to register the real service.
  - The UI's pipe and messages come with `add-onboarding`, and the winlogon agent with its own change.
