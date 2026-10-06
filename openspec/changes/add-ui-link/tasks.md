# Tasks

## 1. UI messages in `ipc`

- [x] 1.1 Split `Direction` into the four per-link directions (D1). Pass the receiving direction in every existing decoder. Verify that all existing `ipc` and `deskhop-agent` tests still pass, and that a new unit test rejects an agent-link type decoded as `UiToService`.
- [x] 1.2 Add the UI types, layouts and codes (D2) as `ipc::ui` (D3 types), and register them in the registry. Verify with unit tests:
  - every UI message round-trips;
  - each is rejected one byte short and one byte long;
  - a 64-byte name, an empty name, non-UTF-8, a 33-byte code, a code containing a space, 257 members, `JoinFailed` reason `8`, side `4` and `up` = `2` are rejected;
  - a UI type on an agent link and an agent type on a UI link are rejected;
  - a 256-member `Status` with 63-byte names encodes within the frame limit.
- [x] 1.3 Extend `crates/ipc/tests/never_panic.rs` and the `decode_ipc` fuzz target to the UI directions. Verify that `cargo test -p ipc` passes and a 20-second fuzz run is clean.
- [x] 1.4 Document the UI link in `docs/protocol.md`: the types table, layouts, codes, and link behavior (status cadence, the 256-frame cap, the console-session rule), and the version-history row noting that version 1 has not shipped. Verify by reading the table against `ipc::message::REGISTRY`.

## 2. UI link ends

- [x] 2.1 Move the handshake, frame writer and bounded frame reader into `link/common.rs` without changing behavior. Verify that every existing link test passes unchanged.
- [x] 2.2 Add `ServiceUiEnd::accept` and `UiEnd::connect` (D3). Verify with duplex tests:
  - `ui_handshake_agrees_version_1`;
  - `agent_role_on_ui_end_is_refused`;
  - `events_arrive_in_order`;
  - `commands_arrive_in_order`;
  - `stalled_ui_is_cut_off`: a 64-byte duplex and a UI that never reads; 1,000 sends return at once, and the link closes past 256 queued frames.

## 3. UI pipe in `win32-session`

- [x] 3.1 Add `create_ui_pipe(name, first, admit)` with the two ACLs (D5), `client_session_id`, and `own_session`. Verify with tests run as a normal user:
  - `ui_pipe_for_own_user_opens`, where `client_session_id` equals this process's session;
  - `ninth_ui_client_is_busy` with `max_instances(8)`;
  - `first_ui_instance_twice_fails`;
  - an SDDL unit test for the interactive-users ACL.

## 4. Service

- [x] 4.1 Add the host's UI hub (D4): pairing state with its time left, `Status` computed from host state, the dirty flag with a 100 ms tick, notices, and UI commands mapped to host actions, including `Join` code parsing and the member check for `RemovePeer`. Extend the host-test harness with UI clients (`UiEnd` over duplex). Verify with host tests:
  - `status_on_connect`;
  - `status_after_peer_down`;
  - `burst_is_coalesced`;
  - `open_pairing_shows_code`;
  - `join_from_ui`;
  - `mistyped_code_is_reported`;
  - `remove_unknown_key_changes_nothing`;
  - `learned_reaches_every_ui`;
  - `stalled_ui_is_dropped`;
  - `malformed_command_closes_only_that_ui`.
- [x] 4.2 Add `ui.rs`, the UI pipe server (D5, D7): first-instance creation with a 5 s retry, at most 8 instances, the session check before `Hello`, and hand-over to the host, in both service and console mode. Verify with:
  - a unit test of the session comparison (equal accepted, different rejected);
  - `name_taken_is_retried`: a test holding the name, then releasing it, sees the server create it within 6 s;
  - a desktop test (`#[ignore]`, console mode, `--data <tmp> --port 0`) where a `UiEnd` client over the real `\\.\pipe\deskhop-ui` gets `Status`, sends `OpenPairing`, and sees the code in the next `Status`.

## 5. Docs and integration

- [x] 5.1 Update `docs/threat-model.md` with a "UI → service (named pipe)" section, covering:
  - who can connect (ACL, console-session check);
  - what a UI can do, and that any console user can pair, as chosen;
  - limits on a slow or hostile UI;
  - pipe squatting and its two mitigations (D6), with the UI-side check left open for `add-onboarding`.
  Verify by reading it against `specs/ui-link`.
- [x] 5.2 Run the full checks:
  - `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` and `cargo archcheck`;
  - the ARM64 check;
  - the Linux set (left to CI if the target is missing locally);
  - the ignored desktop tests in `deskhop-agent` and `deskhop-service`.
  Then run mutation checks, each restored afterwards, and confirm that each fails only its matching tests:
  - accept any role on the UI end;
  - skip the session check;
  - send `Status` on every change without coalescing (caught by `burst_is_coalesced` once its 50 changes were paced over 100 ms and sent from their own task, so arrival times match send times);
  - let a UI type through on the agent link.
