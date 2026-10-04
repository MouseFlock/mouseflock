# Design

## Context

- **`model::PeerMessage`** has eight variants. `Input(InputAction)` carries keys, buttons, wheel and motion. `engine` produces them with `Decision::Send` and consumes them with `Event::FromPeer`.
- **`proto`** owns every wire type and depends on nothing. `proto::pairing` set the pattern: plain structs with `encode_frame`, and `decode(ty, payload)` that never panics and never allocates from a claimed length. `proto::datagram` and `Session::on_datagram` already frame datagrams and drop those that arrive before `Hello`.
- **`transport`:**
  - The node task owns the connection table, including each member connection's `up` flag. One task per connection moves bytes on the control stream.
  - `Internal::Message(id, ty, payload)` takes frames to the node, which dispatches them by role.
  - Nothing reads datagrams yet. quinn's default transport config already allows them in both directions.
- **ADR 0004** set datagrams for motion and streams for everything else, and left the motion layout and coalescing interval open. The design brief set the direction: a sequence number plus running totals.

## Goals / Non-Goals

**Goals:**
- Every requirement in `specs/input-forwarding/spec.md`, tested between real QUIC endpoints on loopback, including a lossy relay.
- Keep the motion logic as pure state machines with time passed in, so loss, reordering, wrap-around and coalescing are unit-tested exactly.
- No change to `engine` or `model`.

**Non-Goals:**
- Running the engine or the node in the service, and mapping `PeerUp`/`PeerDown` to the engine. Those belong to `add-ipc-and-service`.
- Clipboard (`add-text-clipboard`).
- Rate limiting a member's traffic, which is already accepted as residual risk in the threat model.

## Decisions

### D1. Message types and layouts

All integers are little-endian. The codes are:
- side: 0 left, 1 right, 2 top, 3 bottom;
- button: 0 left, 1 right, 2 middle, 3 X1, 4 X2;
- down: 0 released, 1 pressed.

Any other code is a protocol error.

| Type | Name | Channel | Payload | Bytes |
|---|---|---|---|---|
| `0x0006` | `FocusEnter` | stream | side u8, fraction u16, from [32] | 35 |
| `0x0007` | `FocusRefused` | stream | empty | 0 |
| `0x0008` | `EdgeExit` | stream | target [32], side u8, fraction u16 | 35 |
| `0x0009` | `TakenBack` | stream | empty | 0 |
| `0x000A` | `FocusWithdrawn` | stream | empty | 0 |
| `0x000B` | `ForgetSide` | stream | side u8 | 1 |
| `0x000C` | `KeyInput` | stream | mark (12), usage u16, down u8 | 15 |
| `0x000D` | `ButtonInput` | stream | mark (12), button u8, down u8 | 14 |
| `0x000E` | `WheelInput` | stream | mark (12), dx i32, dy i32 | 20 |
| `0x000F` | `SecureAttention` | stream | empty | 0 |
| `0x0010` | `Motion` | datagram | seq u32, total_dx i32, total_dy i32 | 12 |

**Mark:** the same 12 bytes as a `Motion` payload (`seq u32, total_dx i32, total_dy i32`).

`proto::input` holds the structs, `encode_frame`/`encode_datagram`, and `InputMessage::decode(ty, payload)`, all with the crate's no-panic lints. The registry gains the 11 entries, with `Motion` as `Channel::Datagram`.

*Alternatives:*
- One `Input` type with a kind byte. Separate types keep every payload fixed-length and every layout table-checkable.
- Types from `0x0100`. The registry is small, and contiguous numbering reads more simply.

### D2. `proto` stays free of `model`

`proto` defines its own enums for side and button. The mapping between `model::PeerMessage` and the wire structs lives in `transport::forward`, which already depends on both. This keeps `proto` and `model` without new dependencies, as AGENTS.md asks of the audit surface.

### D3. Running totals: the receiver

`motion::Receiver` holds `(seq, dx, dy)`, starting at `(0, 0, 0)` when the peer comes up.

`apply(mark) -> Option<(i32, i32)>` works as follows:
- It accepts `mark` only if `mark.seq.wrapping_sub(self.seq) as i32 > 0`, that is, if it is newer within half the sequence space.
- It returns `(mark.dx.wrapping_sub(self.dx), mark.dy.wrapping_sub(self.dy))`, or `None` if both are 0.
- It then stores `mark`.

Datagrams and the marks on key, button and wheel messages both go through `apply`. Whichever arrives first wins, and the other then reads as not newer.

### D4. Running totals: the sender and coalescing

`motion::Sender` holds the totals, the next sequence number, the time of the last datagram, and whether a repeat is due. Time is a `tokio::time::Instant` passed in.

- `add(dx, dy, now) -> Option<Mark>` adds to the totals. It returns a datagram to send now if 4 ms have passed since the last one; otherwise it sets a flush deadline.
- `poll(now) -> Option<Mark>` handles the deadlines:
  - At the flush deadline it returns the pending totals with the next sequence number.
  - 20 ms after the last datagram, with nothing new since, it returns the same mark once more.
- `mark() -> Mark` is called before a key, button or wheel message, and returns the mark to embed.
  - Any pending movement gets the next sequence number and is cleared, so the stream message alone carries it and no datagram is sent for it. That keeps the 4 ms limit.
  - With nothing pending, it returns the last mark.
  - A mark never arms the 20 ms repeat, because the stream delivers it reliably. Any new movement cancels a pending repeat, and the next datagram arms it again.
- `next_deadline() -> Option<Instant>` tells the connection task when to call `poll`.

Sequence numbers start at 1, so the receiver's initial 0 is always older.

*Alternative:* a fixed 4 ms timer per connection. It wakes up even when idle, and it delays the first movement after stillness by up to 4 ms. Sending the leading edge at once keeps a single movement immediate.

### D5. Where the state lives

**The connection task owns the `Sender` and the `Receiver`,** because both are per connection and run on timers.

- **Node → connection task:** `Out::Peer(PeerMessage)`.
  - The task encodes engine messages.
  - Motion goes to `Sender::add`. Key, button and wheel messages embed `Sender::mark()`.
  - Datagrams are sent with `Connection::send_datagram`. Its errors, such as a full buffer, are ignored: the next datagram or mark covers the movement.
- **When engine messages are allowed (changed during apply).** The first plan was an `Out::Up` from the node when the record exchange finished. That races: a peer that is already up sends its `RecordsDone` and its first engine frame back to back, and the second can reach the task before `Out::Up` does.

  Instead, the task is started knowing whether it is a member connection (from the role, which the ALPN decides), and it watches the control stream for the peer's `RecordsDone`:
  - On a pairing connection, any engine frame closes the connection with `ProtocolError`.
  - On a member connection, an engine frame before the peer's `RecordsDone` closes it with `ProtocolError`. That is the "engine message before the records" case, since a peer only counts itself up after sending its `RecordsDone`, and the stream is ordered.
  - After that frame, messages go to the node on the same FIFO channel the `RecordsDone` went through. So the node has already applied the records and marked the peer up, or decided to close with `5`, before it sees them.
- **Connection task → node:** the task decodes engine frames. For a key, button or wheel message, it first reports any movement from `Receiver::apply(mark)` as `Input(Motion)`, then reports the message.
- **Datagrams:** the task's select loop adds `connection.read_datagram()`.
  - Before the peer's `RecordsDone`, and always on pairing connections, datagrams are dropped.
  - After it, each datagram goes through `Session::on_datagram`, then `proto::input` decoding, then `Receiver::apply`.
  - A malformed datagram closes the connection with `ProtocolError`.
- **Reporting:** the task sends the node `Internal::Peer(id, PeerMessage)`. The node emits `Event::Message { peer, msg }` only if that connection is the peer's current up connection. Otherwise it drops the message: for example, a dialer closed with `5`, or a duplicate being closed under the one-connection rule.
- **`NodeHandle::send(peer, msg)`:** routes to the peer's current up connection, and is dropped silently if there is none. The engine learns of that through `PeerDown`.

*Alternative:* everything in the node task. The node already owns most state, but it would need a timer per connection for coalescing, and `node.rs` is already the largest file. The connection task already runs a select loop with the stream and its out channel; adding datagrams and one deadline fits there.

### D6. Accepted trade-offs in ordering

- **Motion that arrives before its `FocusEnter`.** The datagram can overtake the stream message. The receiver applies the totals while its engine is not yet controlled, so that movement is ignored. This is at most 4 ms of movement right after a crossing, when the cursor has just been placed at the entry point anyway.
- **Motion sent while the receiver's engine ignores it** (not controlled by that peer) is still applied by the receiver, keeping the totals in step. Only the engine discards it.

### D7. Tests

- **`proto` (unit):**
  - round trips for all 11 types;
  - every invalid side, button and down code is rejected;
  - payloads one byte short and one byte long are rejected;
  - `Motion` on a stream and a key message in a datagram are rejected by the registry;
  - the never-panic test and the fuzz targets run over the new types.
- **`transport::motion` (unit, time passed in):**
  - lost, late and duplicate datagrams;
  - wrap-around of totals and of sequence numbers;
  - coalescing (8 movements within 1 ms give at most 2 datagrams);
  - the 20 ms repeat sent exactly once;
  - a mark taking over pending movement, with no datagram sent for it;
  - a mark with nothing pending repeating the last sequence number.
- **`transport::forward` (unit):** `PeerMessage` ↔ wire for every variant.
- **Loopback (`tests/forward.rs`):**
  - every `PeerMessage` variant between two up nodes;
  - "hello" typed in order;
  - a raw client sending `FocusEnter` before `RecordsDone` (close `1`);
  - a key message on a pairing connection (close `1`);
  - an unknown button (close `1`);
  - a click after motion arriving after that motion's delta.
- **Loss test:** 1,000 movements of 1 through a harness `Relay` that drops every third UDP packet in both directions, then a key press. The movements B reports before the key total exactly 1,000. QUIC retransmits the stream, and the key's mark covers lost datagrams.

## Risks / Trade-offs

- **A member can send unbounded deltas.** A member already controls input on any machine it has focus on, so this widens nothing. → The threat model notes that engine messages are accepted only from members that are up.
- **Datagram buffer full under extreme rates.** → Dropping is harmless: the next datagram or mark carries the totals.
- **The 4 ms interval is a guess.** It adds at most 4 ms only when movement events come faster than 250 Hz. → It is one constant (`COALESCE`) next to `REPEAT` (20 ms); revisit after hands-on testing in `add-ipc-and-service`.
- **The version stays 1 while types are added.** This is safe only because version 1 has never shipped, the same reasoning as close reason `5` in `add-discovery-transport`. → `docs/protocol.md` says so in its version history.
