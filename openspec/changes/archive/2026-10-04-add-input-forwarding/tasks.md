# Tasks

## 1. Wire types in `proto`

- [x] 1.1 Add `proto::input` with the 11 message structs and the mark (D1), `encode_frame` for the stream types and `encode_datagram` for `Motion`, and `InputMessage::decode(ty, payload)`, under the crate's no-panic lints. Register the types (`0x0006`–`0x000F` stream, `0x0010` datagram) and update the registry tests. Verify with unit tests:
  - round trips for every type;
  - each invalid side, button and down code rejected;
  - payloads one byte short and one byte long rejected for every fixed layout;
  - the registry rejecting `Motion` in a stream frame and `KeyInput` in a datagram;
  - `version_1` registry test listing all 16 types.
- [x] 1.2 Extend the never-panic test and the `decode_stream` fuzz target to decode and re-encode the new stream types. Extend the existing `decode_datagram` fuzz target to decode and re-encode `Motion`. Verify that `cargo test -p proto` passes and that `cargo fuzz build` succeeds for the targets.
- [x] 1.3 Update `docs/protocol.md`:
  - the message-type table;
  - an "Engine messages" section with every layout and code table;
  - a "Motion" section: running totals, sequence comparison by wrapping difference, the 4 ms coalescing, the 20 ms repeat, and the mark on key, button and wheel messages;
  - the rule that engine messages need an up member connection;
  - the version-history row, noting version 1 is still unreleased.

  Verify that every layout in the doc matches the byte counts in the 1.1 tests.

## 2. Motion state machines in `transport`

- [x] 2.1 Add `transport::motion::Receiver` (D3). Verify with unit tests:
  - lost datagram (5, 5, 5 with the middle one lost totals 15);
  - late datagram ignored;
  - duplicate ignored;
  - zero movement not reported;
  - totals wrapping past `i32::MAX`;
  - sequence numbers wrapping past `u32::MAX`.
- [x] 2.2 Add `transport::motion::Sender` (D4) with `COALESCE` = 4 ms and `REPEAT` = 20 ms, taking time as an argument. Verify with unit tests:
  - the first movement is sent at once;
  - 8 movements within 1 ms give at most 2 datagrams, with totals ending at 8;
  - a pending flush is sent exactly 4 ms after the previous datagram;
  - the repeat is sent once, 20 ms after the last datagram, and not again;
  - `mark()` takes over pending movement with a new sequence number and no datagram;
  - `mark()` with nothing pending returns the last sequence number;
  - sender and receiver driven together through random loss converge on the same totals.

## 3. Forwarding in the transport

- [x] 3.1 Add `transport::forward`: mapping between `model::PeerMessage` and the `proto::input` structs for every variant, with `Input(Motion)` handled by the sender rather than encoded. Verify with a unit test that every variant except motion round-trips through its frame.
- [x] 3.2 Extend the connection task (D5):
  - `Out::Peer`, and the task told whether it is a member connection;
  - the sender and receiver;
  - datagram reading through `Session::on_datagram`;
  - engine frames decoded in the task, closing with `ProtocolError` on a pairing connection or before the peer's `RecordsDone`, with datagrams dropped in both cases;
  - the deadline from `Sender::next_deadline` in the select loop.

  Add `NodeHandle::send`, `Event::Message` and `Internal::Peer`; the node reports a message only from the peer's current up connection. Verify with loopback tests in `tests/forward.rs`:
  - `every_message_crosses`: each `PeerMessage` variant from A reported by B with A's id;
  - `typing_keeps_order`: "hello" arrives as ten key messages in order;
  - `send_to_a_peer_that_is_down`: dropped without error.
- [x] 3.3 Enforce where engine messages may travel. Verify with loopback tests using the harness raw client:
  - `engine_message_before_records`: `FocusEnter` after `Hello` but before `RecordsDone`, closed with `1`;
  - `engine_message_on_a_pairing_connection`: a key message on `deskhop-pair/1`, closed with `1`;
  - `unknown_button`: closed with `1`;
  - `motion_before_up_is_dropped`: a `Motion` datagram before the record exchange, after which the connection still comes up.
- [x] 3.4 Verify motion end to end. Add a lossy mode to the harness `Relay` that drops every third UDP packet in both directions. Verify with loopback tests:
  - `click_lands_after_its_motion`: motion then a button press, with B reporting the movement before the press;
  - `motion_survives_loss`: 1,000 movements of 1 through the lossy relay, then a key press, with B's reported movement before the key totalling exactly 1,000.
- [x] 3.5 Update `docs/threat-model.md`:
  - engine messages and `Motion` are accepted only on member connections that are up, and are protocol errors otherwise;
  - a member can drive input only on a machine whose engine has handed it focus, and widens nothing it could not already do;
  - datagrams before up are dropped.

  Update `docs/design-brief.md` to replace the motion-layout open question with a pointer to `docs/protocol.md`. Verify that the documents name the same type numbers as the registry.

## 4. Final checks

- [x] 4.1 Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo archcheck`, the Linux test set (`cargo test -p proto -p model -p engine -p policy -p pairing`) and the ARM64 check. Then break one rule at a time and confirm that only the matching tests fail:
  - accept a non-newer sequence number in `Receiver::apply`;
  - skip applying the mark before a button press;
  - accept engine frames before the peer's `RecordsDone`;
  - send a datagram for every movement, with no coalescing.

  Restore from copies and touch the files before the final run.
