# Design

## Context

- **Transport:** every member connection has one bidirectional control stream plus `Motion` datagrams.
  - `conn::run` accepts no other stream. A stream a peer opens sits unread.
  - Frames are at most 65,536 bytes of payload (`proto::frame::MAX_FRAME_PAYLOAD`).
  - Engine frames are recognised by a hard-coded type range (`is_engine_frame`, `0x0006`–`0x000F`), and anything else on the control stream is decoded as pairing. A new control message must be routed explicitly.
- **Messages that can't carry text:** `model::PeerMessage` and `ipc::message::ToAgent` are `Copy`, with fixed layouts.
  - Clipboard text cannot ride on either without dropping `Copy` and changing every user.
  - It does not need to: clipboard is not an engine message.
- **The host** (`bin/service/src/host.rs`) already sees every focus change as `Decision::FocusChanged(FocusView)`, with views `Local`, `Forwarding(peer)` and `Controlled(peer)`. It already handles `PeerUp`/`PeerDown`, and has the agent link's `FromAgent` stream.
- **The agent** (`bin/agent/src/session.rs`) runs a current-thread tokio loop over service commands and captured events.
  - `win32-input` owns two Win32 threads with hidden windows (input, screen); their helpers are `pub(crate)`.
  - `win32-clipboard` is an empty crate that the agent already depends on.
- **Versions:** `PROTOCOL_VERSION` and `IPC_VERSION` are 1, not shipped. `docs/protocol.md` records version 1 as filled in without a bump.
- **Settled with the user:** text follows the cursor (announce on copy, fetch on arrival); no off switch now.

## Goals / Non-Goals

**Goals:**
- Every requirement in `specs/clipboard` and the added `specs/agent-link` requirements.
- Keep the engine untouched: the clipboard is driven by focus changes the host already sees.
- Keep input latency unaffected by clipboard size.

**Non-Goals:**
- Images, files, rich text.
- Delayed rendering: fetching when an app pastes.
- Clipboard on the secure desktop.
- An off switch or policy.

## Decisions

### D1. The tracker: a pure state machine in `bin/service/src/clipboard.rs`

```rust
struct Tracker {
    newest: Option<Copy>,                 // Copy { owner: Owner, id: u64 }, Owner = Local | Member(PeerId)
    held: Option<(u64, Arc<str>)>,        // this machine's own shareable text, by ID
    holding: Option<Copy>,                // the copy now on this machine's clipboard
    fetch: Option<(Copy, Instant)>,       // in flight, with its deadline
}
```

| Input | Effect |
|---|---|
| `local_text(text, id)` | `held` = text, `newest` = `holding` = local copy. Out: offer `id` to all up members. |
| `local_not_shared` | `held` = `None`, `newest` = `holding` = `None`; any fetch is abandoned. Out: nothing. |
| `offer(peer, id)` | `newest` = that member's copy. |
| `arrived(up)` | If `newest` is a member's copy, ≠ `holding`, and its owner is in `up` (and no fetch for it is in flight): start a fetch. Out: `ClipboardRequest(id)` to that member. |
| `request(peer, id)` | If `held` has that ID: out: send the text to `peer`. Otherwise nothing. |
| `received(peer, id, text)` | If it matches `fetch` and `newest`: `holding` = that copy, fetch cleared. Out: set the text in the agent. Otherwise discard. |
| `peer_down(peer)` | A fetch from that peer is abandoned. |

A fetch past its 10 s deadline is abandoned lazily, by the next `arrived`, which may then try again. No timer is needed.

- **Arrival** is computed in the host from consecutive `FocusView`s. It is any change into `Controlled(_)`, or `Forwarding(_)` → `Local`. The function `arrived(previous, next) -> bool` is unit-tested.
- **IDs** are random `u64`s from `rand_core::OsRng`; the service already depends on `rand_core`.
  - A clash between two copies is not a safety problem: the worst case is a stale text being served for a request.
- **Tests:** a unit test per row and per spec scenario, run as sequences, in the style of the engine's replay tests. The clipboard is not engine behavior, so it lives beside the host.

*Alternative:* put the tracker in `engine`. That adds clipboard events to the audit-surface crate and a variant to the `Copy` `PeerMessage`, for logic that needs no input state. Rejected.

### D2. Wire messages (peer protocol version 1, unshipped)

| Type | Name | Channel | Payload |
|---|---|---|---|
| `0x0011` | `ClipboardOffer` | control stream | id: u64 |
| `0x0012` | `ClipboardRequest` | control stream | id: u64 |
| `0x0013` | `ClipboardData` | clipboard stream | id: u64, len: u32 (≤ 4,194,304) |
| `0x0014` | `ClipboardChunk` | clipboard stream | 1–65,536 bytes |

- **Registry:** `Channel` gains `Transfer` (clipboard streams). `check` rejects a control type on a transfer stream and the reverse.
- **New module `proto::clipboard`:**
  - `ClipboardMessage` (offer, request) decodes and encodes like `InputMessage`;
  - `TransferReader` is a sans-I/O checker: each frame gives `Started { id, len }` or `Chunk(&[u8])`, and `finish()` at the end of the stream. It enforces exactly one `ClipboardData` first, non-empty chunks summing to exactly `len`, and `len` ≤ 4 MiB.
  - Like every `proto` decoder, it buffers nothing. It lends chunks back, and `transport` collects them (growing as bytes arrive, never by the claimed `len` up front) and checks UTF-8 with `proto::clipboard::text` at the end.
  - Both are under `proto`'s no-panic lints, and covered by the never-panic test and the `decode_stream` fuzz target (extended to feed frames through `TransferReader` too).

### D3. Transport

- **Routing:** `conn.rs` routes `0x0011`/`0x0012` on the control stream (after `RecordsDone`) to `Internal::Clipboard`, beside the engine range. Before the peer is up, or on a pairing connection, they are a protocol error, as for engine frames.
- **Incoming clipboard streams:** `conn::run` also selects on `connection.accept_uni()`.
  - Each accepted stream is read by a spawned task through `TransferReader`. It forwards `Complete` as `Internal::ClipboardText`; an error closes the connection with `ProtocolError`.
  - The endpoint's transport config sets `max_concurrent_uni_streams(1)` and `max_concurrent_bidi_streams(1)` (the control stream only). A peer cannot open a second clipboard stream until the first ends.
  - On a pairing connection, any uni stream is a protocol error. On a member connection, one accepted before the peer's `RecordsDone` is held unread until it arrives. QUIC does not order streams, so a correct peer that counts this machine up can open one before its `RecordsDone` is read here.
- **Outgoing:** `NodeHandle::send_clipboard(peer, id, Arc<str>)` makes the connection task spawn a writer. It calls `open_uni`, sets the stream's priority below the control stream's (`set_priority(-1)`), writes `ClipboardData` then 64 KiB chunks, and finishes the stream. The control stream is never touched, so input frames are never queued behind text, and QUIC sends waiting input first (spec: "Transfers never delay input").
- **New `NodeHandle` calls:** `offer_clipboard(peer, id)` and `request_clipboard(peer, id)` send on the control stream; only to up peers, as `send` does.
- **New `Event` variants:** `ClipboardOffer { peer, id }`, `ClipboardRequest { peer, id }` and `ClipboardText { peer, id, text: Arc<str> }`, emitted only for the peer's current up connection.
- **Memory:** at most one incoming transfer (≤ 4 MiB plus a frame) per connection, plus the held copy.

### D4. Agent link (IPC version 1, unshipped)

| Type | Name | Direction | Payload |
|---|---|---|---|
| `0x0016` | `ClipboardText` | agent → service | len: u32 (≤ 4,194,304) |
| `0x0017` | `ClipboardChunk` | agent → service | 1–65,536 bytes |
| `0x0018` | `ClipboardNotShared` | agent → service | reason: u8 (`0` private, `1` no text, `2` too large) |
| `0x0027` | `SetClipboard` | service → agent | len: u32 (≤ 4,194,304) |
| `0x0028` | `SetClipboardChunk` | service → agent | 1–65,536 bytes |

- **Text-carrying types:** `ToService` gains `ClipboardText(u32)`, `ClipboardChunk(Vec<u8>)` and `ClipboardNotShared(reason)`; it is already `Clone`.
  - `ToAgent` stays `Copy`. The two service → agent clipboard types go in a separate `ToAgentClipboard` enum, and the agent decoder returns `enum FromService { Command(ToAgent), Clipboard(ToAgentClipboard) }`. Existing matches on `ToAgent` are unchanged.
- **Runs are assembled at the ends,** not in the codec. A shared `RunAssembler` (`ipc::clipboard`) enforces the agent-link run rules: start, exact total, UTF-8, no second start mid-run, no stray chunk.
  - The service end yields `FromAgent::Clipboard(Clipboard::Text(String) | Clipboard::NotShared(reason))` only for a complete run.
  - The agent end yields `SetClipboard(String)`.
  - A breach closes the link (spec: "Clipboard runs on the agent link").
- **Interleaving:** other messages, input included, may come between chunks. The service's `Held` queue (until the first `Pong`) holds clipboard messages like any other.
- **Queue limits:** a 4 MiB run is 64 chunks, well under the agent link's 4,096-command cap. The UI link is unaffected.

### D5. `win32-clipboard`

A thread `deskhop-clipboard` with its own hidden top-level window (registered class, `WS_EX_TOOLWINDOW`, not `HWND_MESSAGE`, matching `win32-input`'s choice) and message loop.

- **Listening:** `AddClipboardFormatListener(hwnd)`. On `WM_CLIPBOARDUPDATE` it (re)arms a 100 ms timer; when the timer fires it reads once. That gives one report per settled change.
- **Own writes:** after setting the clipboard, the thread records `GetClipboardSequenceNumber()`. An update whose sequence number equals the recorded one is ignored.
- **Reading:**
  - `OpenClipboard(hwnd)`, retried every 20 ms for up to 1 s, because another app may hold it.
  - It checks the privacy markers first (`RegisterClipboardFormatW` for `ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory` and `CanUploadToCloudClipboard`). Presence of the first, or a DWORD 0 in either of the others, means private, and the text is not read.
  - Then `CF_UNICODETEXT`: `GlobalLock`, up to the first NUL, converted to UTF-8.
  - If the UTF-16 size already exceeds 4 MiB × 2, or the UTF-8 result exceeds 4 MiB, it is too large. No `CF_UNICODETEXT` means no text.
- **Writing:** `OpenClipboard` (same retry), `EmptyClipboard`, `SetClipboardData(CF_UNICODETEXT, GlobalAlloc'd UTF-16 + NUL)`, `CloseClipboard`, then record the sequence number. The handle's ownership passes to Windows on success; on failure it is freed.
- **API:** `start() -> (Clipboard, Receiver<Copied>)`, where `Copied = Text(String) | NotShared(Reason)`, and `Clipboard::set(String)`, which posts to the thread. Stop is `WM_QUIT`, as for the input workers.
- **Placement:** every `unsafe` block carries `// SAFETY:`. The crate is linked only by `bin/agent`, as already enforced.

The pure parts are unit-tested without the clipboard: the marker decision from (present, value) triples, UTF-16 → UTF-8 up to NUL with size checks, and the chunking. The real clipboard is covered by `#[ignore]` desktop tests:
- set, then read back through the listener, and confirm no report of the agent's own write;
- a copy with `CanUploadToCloudClipboard` = 0 is private;
- Notepad-sized text round-trips.

### D6. Agent wiring

`session.rs` starts the clipboard worker after capture and adds two arms to its `select!`:
- `Copied` → `ClipboardText` run or `ClipboardNotShared`, sent through `ServiceHandle`; the run is interleaved naturally by the writer's queue;
- `FromService::Clipboard(SetClipboard(text))` → `clipboard.set(text)`.

### D7. Host wiring

- **Agent:** `FromAgent::Clipboard(Text)` → `tracker.local_text(text, random id)` → `node.offer_clipboard(p, id)` for every up `p`. `NotShared` → `tracker.local_not_shared()`.
- **Focus:** in `apply`, `Decision::FocusChanged(v)` compares with the last view; on arrival → `tracker.arrived(&up)` → `node.request_clipboard`.
- **Transport:** `Event::ClipboardOffer/Request/Text` → tracker → `node.send_clipboard` / `to_agent_clipboard(SetClipboard(text))`.
- **Peers:** `PeerDown` → `tracker.peer_down`.
- **Status:** clipboard events don't mark the UI status dirty; nothing in `Status` changes.
- **Logging:** "clipboard: offered N bytes", "fetched N bytes from <name>", "set N bytes", "copy not shared (private)". Never the text. `log_has_no_input` gains a clipboard check.
- **Console mode:** prints nothing new; a `clipboard` command is not needed.

### D8. Tests

- **`proto`:**
  - round trips;
  - lengths 1 byte short and 1 byte long;
  - `TransferReader`: exact total, over-limit header rejected before any chunk, extra chunk, short stream, bad UTF-8, a second `ClipboardData`;
  - channel checks;
  - never-panic over random frames.
- **`transport`** (loopback, two nodes):
  - `clipboard_offer_reaches_up_peer`;
  - `text_transfers_on_its_own_stream`: 4 MiB arrives intact;
  - `input_not_delayed_by_transfer`: while a 4 MiB transfer runs, 100 engine messages arrive within the time they take without it, plus a margin;
  - `second_stream_waits`;
  - `oversized_transfer_closes`, by hand-built frames through a test hook;
  - `offer_before_up_is_protocol_error`.
- **`ipc`:**
  - round trips and limits;
  - the run assembler: stray chunk, short run, interleaved input accepted, second start rejected, over-limit;
  - never-panic, plus the fuzz target extended.
- **`bin/service`:**
  - tracker unit sequences (every D1 row and spec scenario);
  - `arrived`;
  - host tests with two hosts and stand-in agents: `copy_then_cross_sets_text_on_b`, `copy_on_b_then_cross_back`, `private_copy_is_not_offered`, `stale_request_gets_nothing`, `no_echo_after_set`, and `log_has_no_clipboard_text`.
- **`win32-clipboard`:** the pure unit tests, plus the `#[ignore]` desktop tests in D5.
- **Manual:**
  - this PC: the agent in console mode sets and reports text; a password-manager-style copy is private;
  - two PCs: deferred with add-onboarding 7.3.

### D9. Docs

- **`docs/protocol.md`:**
  - the four peer types with layouts;
  - "Clipboard streams" under Transport: when they open, one at a time, the frame order, limits;
  - the five IPC types and the run rules;
  - the version-history rows: still version 1, unshipped.
- **`docs/threat-model.md`:** a "Clipboard" section:
  - **On the LAN:** text travels only on member connections (QUIC, pinned identities), only after a request, only for the newest shareable copy.
  - **Private copies:** they stay on the machine.
  - **Memory:** the service holds at most the held copy plus one transfer per connection, never on disk or in logs.
  - **A member can request the held copy whenever it likes:** a member could already type and read the screen, so this is no new power; the request still only yields the newest shareable copy.
  - **Hostile input:** a hostile agent or peer is bounded by the 4 MiB limits and the no-panic decoders.
  - **Residual:** text lands in the console user's clipboard on the target, even if a different Windows user is logged on there.

## Risks / Trade-offs

- **Paste right after crossing can see the old text.** The fetch takes a LAN round trip plus the transfer (≈ milliseconds for normal copies). → Accepted, as agreed. Delayed rendering could close it later.
- **Pasting at a PC's own keyboard without crossing gets the old text.** → Accepted, as agreed. Out of scope.
- **Apps that hold the clipboard open.** → 1 s retry window, then the read or write is skipped. A skipped read means that copy is not offered, and the next copy is.
- **Clipboard history and cloud sync on the receiving PC** treat received text like a local copy. → Matches the user having copied it there. Private copies are never sent, so nothing marked private is re-exposed.
- **Random-ID collision.** → 2⁻⁶⁴. The worst case is serving the newest shareable copy to a member that asked for an older one.
- **The listener's hidden window and timer** add one thread to the agent. → Same pattern as the screen thread.

## Migration Plan

Nothing shipped. Both protocols stay at version 1, recorded in `docs/protocol.md`. The registered dev service picks up the change on the next `tools/dev/service.ps1 install`.
