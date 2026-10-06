# Tasks

## 1. Wire messages in `proto`

- [x] 1.1 Add `Channel::Transfer` and the four types `0x0011`–`0x0014` to the registry (D2), and update `version_1_defines_exactly_these_types`. Verify with registry tests:
  - sorted and unique;
  - offer and request accepted on the control stream and rejected on a transfer stream;
  - data and chunk accepted only on a transfer stream;
  - `0x0015` unknown.
- [x] 1.2 Add `proto::clipboard` with `ClipboardMessage` (offer and request) and `TransferReader` (D2). Verify with unit tests:
  - round trips;
  - each fixed layout one byte short and one byte long;
  - `TransferReader` accepts an exact total, including empty text and exactly 4,194,304 bytes;
  - it rejects a `len` of 4,194,305 at the header, an extra chunk, a missing chunk at end of stream, a zero-byte chunk, invalid UTF-8, and a second `ClipboardData`;
  - it lends each chunk's bytes back without buffering them; the caller collects the text.
- [x] 1.3 Extend `crates/proto/tests` never-panic and the `decode_stream` fuzz target to `ClipboardMessage` and `TransferReader`. Verify that `cargo test -p proto` passes and a 20-second fuzz run is clean.
- [x] 1.4 Document the four types, their layouts, and "Clipboard streams" in `docs/protocol.md`, and update the peer version-history row. Verify by reading the table against `proto::registry::REGISTRY`.

## 2. Clipboard streams in `transport`

- [x] 2.1 Route `ClipboardOffer` and `ClipboardRequest` on the control stream to new `Event` variants, and add `NodeHandle::offer_clipboard` and `request_clipboard` (D3). They are only sent to up peers, and before up they are a protocol error. Verify with loopback tests:
  - `clipboard_offer_reaches_up_peer`;
  - `clipboard_request_reaches_up_peer`;
  - `offer_before_up_is_protocol_error`, through the existing hand-built-frame test path;
  - `offer_on_a_pairing_connection_is_protocol_error`;
  - `chunk_on_the_control_stream_is_protocol_error`.
- [x] 2.2 Accept and read incoming uni streams through `TransferReader`, add `NodeHandle::send_clipboard` (open uni, priority below the control stream, `ClipboardData`, 64 KiB chunks, finish), and set the endpoint's stream limits (1 uni, 1 bidi) (D3). Verify with loopback tests:
  - `text_transfers_on_its_own_stream` (4 MiB intact, and a 3-byte text);
  - `second_stream_waits`;
  - `oversized_transfer_closes`;
  - `short_or_invalid_transfer_closes`;
  - `uni_stream_on_a_pairing_connection_is_protocol_error`;
  - `early_stream_is_held_until_records`.
- [x] 2.3 Prove transfers don't delay input. In the test `input_not_delayed_by_transfer`, a member sends 100 `KeyInput` messages alone, then opens a 4 MiB transfer and sends half of it, so it is truly still under way, and sends 100 more. All the keys must arrive before the text, and within the first time plus 50 ms. Verify that it passes 10 runs in a row, and fails when a temporary change reads clipboard streams inside the control-stream loop. Sending on the control stream cannot be tested this way, because the receiver rejects it.
## 3. Agent-link messages in `ipc`

- [x] 3.1 Add `0x0016`–`0x0018` and `0x0027`–`0x0028` (D4): `ToService` variants, the `ToAgentClipboard` enum, and the agent decoder's `FromService`, keeping `ToAgent` `Copy`. Verify with unit tests:
  - round trips;
  - one byte short and one byte long;
  - `len` 4,194,305 rejected;
  - a zero-byte chunk rejected;
  - reason `3` rejected;
  - each type rejected in the wrong direction and on the UI link.
- [x] 3.2 Add `ipc::clipboard::RunAssembler`, and wire it into both link ends (service end → `FromAgent::Clipboard`, agent end → `SetClipboard(String)`), including the `Held` queue before the first `Pong`. Verify with duplex tests:
  - `copy_run_reaches_service`;
  - `stray_chunk_closes_link`;
  - `short_run_then_new_run_closes_link`;
  - `input_between_chunks_is_accepted`;
  - `invalid_utf8_closes_link`;
  - `set_clipboard_reaches_agent`.
- [x] 3.3 Extend `never_panic.rs` and the `decode_ipc` fuzz target to the new types and the assembler. Verify that `cargo test -p ipc` passes and a 20-second fuzz run is clean.
- [x] 3.4 Document the five IPC types and the run rules in `docs/protocol.md`, and update the IPC version-history row. Verify by reading the table against `ipc::message::REGISTRY`.

## 4. `win32-clipboard`

- [x] 4.1 Add the pure helpers:
  - the privacy decision from the three markers;
  - UTF-16 → UTF-8 up to the first NUL, with the 4 MiB checks;
  - UTF-8 → NUL-terminated UTF-16.

  Verify with unit tests: each marker alone, value 0 versus 1, none; text with an embedded NUL; exactly 4 MiB versus one byte over; surrogate pairs.
- [x] 4.2 Add the `deskhop-clipboard` thread (D5): hidden window, `AddClipboardFormatListener`, the 100 ms settle timer, open-retry for up to 1 s, reading with the markers checked first, writing, and the sequence-number check for its own writes. Export `start() -> (Clipboard, Receiver<Copied>)` and `Clipboard::set`. Every `unsafe` block carries `// SAFETY:`. Verify with `#[ignore]` desktop tests:
  - `set_is_not_reported`;
  - `copy_is_reported_once` (three updates within 50 ms give one report);
  - `private_copy_is_reported_private` (a copy with `CanUploadToCloudClipboard` = 0);
  - `copy_without_text_is_no_text` (only a non-text format);
  - `text_round_trips` (with non-ASCII and CRLF).
  - Also run `cargo clippy -p win32-clipboard -- -D warnings`.

## 5. Agent wiring

- [x] 5.1 Start the clipboard worker in `bin/agent/src/session.rs`, send each `Copied` as a run or `ClipboardNotShared`, and apply `SetClipboard` (D6). Verify with an `#[ignore]` agent test against a fake service end on a real pipe:
  - `Set-Clipboard hello` in PowerShell yields a `ClipboardText` run of `hello`;
  - `SetClipboard("world")` makes `Get-Clipboard` return `world`, with no report back.

## 6. Service: tracker and host

- [x] 6.1 Add `bin/service/src/clipboard.rs`: the `Tracker` (D1) and `arrived(previous, next)`. Verify with unit tests for every D1 row and every `specs/clipboard` scenario as sequences:
  - copies on two machines;
  - own copy after an offer;
  - nothing newer;
  - owner down;
  - stale request;
  - private after shareable;
  - no echo;
  - newer copy during a transfer;
  - fetch timeout then retry on the next arrival;
  - `arrived` for each focus transition.
- [x] 6.2 Wire the tracker into the host (D7): agent clipboard events, offers on copy, arrival on focus changes, requests and sends, received text to the agent, and peer down. Fetch timeouts are checked at the next arrival, so no timer is needed. Log lengths only. Verify with host tests over loopback with stand-in agents:
  - `copy_then_cross_sets_text_on_b`;
  - `copy_on_b_then_focus_returns_to_a` (B's user takes back; A goes from forwarding to local);
  - `private_copy_is_not_offered`;
  - `only_the_newest_copy_is_sent` (stale requests are covered by the tracker's unit test `stale_request_gets_nothing`);
  - `no_echo_after_set`;
  - `log_has_no_clipboard_text` (both logs, after a transfer of a distinctive string).
- [x] 6.3 Update `docs/threat-model.md` with the "Clipboard" section (D9), and the assets line. Verify by reading it against `specs/clipboard` and the code paths in 2.2 and 6.2.

## 7. Integration

- [x] 7.1 Run the full checks, and verify that each passes:
  - `cargo fmt --all --check`, `cargo archcheck`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`;
  - the Linux set (`cargo test -p proto -p model -p engine -p policy -p pairing -p ipc`), on Windows here and in CI;
  - the ARM64 check;
  - the ignored desktop tests in `win32-clipboard`, `deskhop-agent` and `deskhop-service`;
  - 20-second runs of `decode_stream` and `decode_ipc`.
- [x] 7.2 Manual check with the user on this PC, with `deskhop-service --console`:
  - copy text in Notepad; the console log shows an offer with its length and no text;
  - copy from a password manager (or `Set-Clipboard` via an app that sets `CanUploadToCloudClipboard` = 0); the log shows it was not shared;
  - copy an image; the log shows no text.
- [ ] 7.3 Manual check with the user on two PCs, deferred with add-onboarding 7.3:
  - copy on A, cross to B, paste;
  - copy on B, cross back, paste on A;
  - type on B while a large copy transfers, with no lag;
  - a password-manager copy does not reach B.

  If a second PC is not available, record that and leave this task open.

  Deferred by the user on 2026-10-06, with add-onboarding 7.3, until the rest of the product is complete. On one PC, 7.2 checked the agent's copies, private and non-text cases. The host tests over loopback cover copy, cross and fetch in-process.
