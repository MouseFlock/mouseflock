# Proposal

## Why

The engine decides when focus crosses to another machine and what to forward. `transport` connects members and keeps them up. `win32-input` captures and injects input. But the engine's `PeerMessage`s have no wire encoding, and the transport has no way to carry them. This change defines that encoding and carries it, so that `add-ipc-and-service` only has to connect the engine to the node to get the cursor crossing between two PCs. It also settles an open question from ADR 0004 and the design brief: the motion datagram layout and coalescing.

## What Changes

- **Engine messages on the wire.** Every `model::PeerMessage` gets a message type and a fixed payload:
  - focus messages: `FocusEnter`, `FocusRefused`, `EdgeExit`, `TakenBack`, `FocusWithdrawn`;
  - layout and security messages: `ForgetSide`, `SecureAttention`;
  - forwarded input: one type each for key, button and wheel.

  They travel on the control stream of a member connection, in order, once the peer is up.
- **Motion on datagrams, as running totals.**
  - Forwarded motion becomes a `Motion` datagram carrying a sequence number and the running totals of horizontal and vertical movement since the connection came up. The receiver turns totals back into movement.
  - A lost datagram loses no movement, because the next one carries it.
  - A reordered datagram is dropped by its sequence number.
- **Coalescing.** Each connection sends at most one `Motion` datagram every 4 ms. Movement that arrives sooner is added to the next one. When movement stops, the last totals are sent once more 20 ms later, in case the final datagram was lost.
- **Clicks land where the cursor is.** Every key, button and wheel message carries the sender's motion totals at that moment. Before acting on it, the receiver applies any movement it has not yet seen. A click therefore never arrives ahead of the movement before it, even when the datagram carrying that movement is lost or late.
- **Only between members that are up.**
  - An engine message on a pairing connection, or on a member connection before the record exchange completes, is a protocol error.
  - `Motion` datagrams that arrive before then are dropped.
- **Transport API.** `NodeHandle::send(peer, PeerMessage)` sends a message, and the node reports `Event::Message { peer, msg }`. Motion arrives as `PeerMessage::Input(Motion { dx, dy })`, with the movement rebuilt from the totals.
- **Wire protocol.** Version 1 gains message types `0x0006`–`0x000F` (stream) and `0x0010` (`Motion`, datagram). `PROTOCOL_VERSION` stays 1, because version 1 has never shipped. `docs/protocol.md` gains the layouts and the motion rules.

## Capabilities

### New Capabilities
- `input-forwarding`: carrying engine messages and forwarded input between members. It covers which connections may carry them, their order, motion as running totals on datagrams, coalescing and the final repeat, the motion mark on key, button and wheel messages, and loss and reordering.

### Modified Capabilities
- `wire-protocol`: "Message type registry" lists the version 1 types, and that list now includes the engine messages and the `Motion` datagram type.

## Impact

- **Crates:**
  - **`proto`:**
    - Wire structs for the 11 new message types and their encoding and decoding, following the pattern of `proto::pairing`.
    - The never-panic test and the `decode_stream` fuzz target cover them, and a `decode_datagram` fuzz target covers `Motion`.
    - No new dependencies: `proto` still does not depend on `model`.
  - **`transport`:**
    - Mapping between `model::PeerMessage` and the wire structs.
    - Pure motion sender and receiver state machines (coalescing, running totals, marks).
    - Datagram reading in the connection task, `NodeHandle::send`, and `Event::Message`.
- **Untouched:** `engine` and `model`. `PeerMessage` is used as it is.
- **Dependency rules hold:** `engine` reaches no `transport`, `ipc` or `win32-*`; `proto` and `model` gain no dependencies; no new `unsafe`; no `cfg` branches for other operating systems.
- **No grid, no server/client toggle, no IP address field.** Nothing is user-facing.
- **Docs:**
  - `docs/protocol.md`: the new message types, their layouts, and motion and coalescing.
  - `docs/threat-model.md`: engine messages are accepted only from members that are up, and what a member can do with them.
  - `docs/design-brief.md`: the motion-layout open question is answered.
- **Later changes:**
  - `add-ipc-and-service` runs the engine and the node in the service and connects `Decision::Send` and `Event::Message`. It also maps `PeerUp`/`PeerDown` to the engine's `PeerConnected`/`PeerLost`, and passes `Inject`, `SetCapture` and `WarpCursor` to the agent. That is when the cursor first crosses between two PCs.
