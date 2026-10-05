# Tasks

## 1. Codec in `ipc`

- [x] 1.1 Add the frame codec (D1): encode, plus an incremental decode that returns need-more, a frame, or an error from the header alone. Add `ipc`'s no-panic lints (as in `proto`), and its dependencies `model` and `tokio` (`io-util`, `sync`, `time`, `rt`, `macros`). Verify with unit tests for the spec's frame bytes, a 65,537-byte header rejected with 6 bytes of input, and need-more at every split of a valid frame. Check that `cargo archcheck` passes and that `cargo tree -p engine` shows no `ipc`.
- [x] 1.2 Add `Hello` and the registry with direction and version (D1, D2). Add every agent-link message, its layout and code tables, mapping to and from `model` types, and the screen limits. Verify with unit tests:
  - every message round-trips;
  - each message is rejected one byte short and one byte long;
  - undefined origin, down, button and mode codes are rejected;
  - a type received from the wrong direction is rejected;
  - an unknown type and type `0x0000` are rejected;
  - `Hello` with 6 trailing bytes is accepted, and its exact bytes for a session agent are `01 00 01 00 01`;
  - each screen limit in D6 is rejected, and a valid 32-monitor screen with 256-byte ids is accepted.
- [x] 1.3 Add `ClockSync` (D4). Verify with unit tests: the spec scenario (offset 50,000, event 51,500 → 1,500), a smaller rtt replacing the estimate, a larger one ignored, and conversion clamping at 0.
- [x] 1.4 Add `crates/ipc/tests/never_panic.rs` (random and mutated frames through both ends' decoders, with every decoded message re-encoding to the same bytes) and a `decode_ipc` fuzz target, and add that target to the nightly fuzz matrix. Verify that `cargo test -p ipc` passes, the fuzz target builds, and a 20-second run is clean.

## 2. Async ends

- [x] 2.1 Add `ServiceEnd::accept` and `AgentEnd::connect` (D3): the handshake with its 5 s timeout, the reader and writer tasks, the bounded frame buffer, the 4,096-frame outbound cap, and closing on errors. The agent end answers `Ping`s itself. Verify with duplex tests:
  - `handshake_agrees_version_1`;
  - `wrong_role_is_refused` (a UI `Hello` on the service end);
  - `versions_do_not_overlap`;
  - `silent_peer_times_out` (paused time);
  - `commands_arrive_in_order`;
  - `protocol_error_closes_the_link`, where both receivers yield `None`.
- [x] 2.2 Add the service end's clock handling (D4): `Ping` at start and every 10 s, events held until the first `Pong`, times converted, and the 10,000-event hold cap. Verify with duplex tests on paused time:
  - `events_are_converted_to_the_service_clock`;
  - `events_wait_for_the_first_pong` (order kept);
  - `pings_every_ten_seconds`.
- [x] 2.3 Verify that a stalled agent cannot block the service. Add duplex test `stalled_agent_does_not_block`: a 64-byte duplex with an agent that never reads; 1,000 `send`s return without waiting, events from the agent keep arriving, and passing 4,096 queued frames closes the link.
- [x] 2.4 Add `ipc` to the Linux CI test step and to the Linux command in `AGENTS.md`, because the crate has no Windows code. Verify with `cargo check -p ipc --target x86_64-unknown-linux-gnu` if that target is installed locally; otherwise rely on the CI Linux job, and say so in the PR.

## 3. The session agent

- [x] 3.1 Add `deskhop-agent --session --pipe <name>` (D5):
  - parse arguments, keeping `--trace` and `--winlogon`;
  - connect with the busy retry;
  - handshake;
  - start capture only after the handshake;
  - bridge events, apply commands, and log local command failures;
  - on link end, drop capture first, then exit 0;
  - exit codes 3, 4 and 5 as in D5.

  Verify with argument-parsing unit tests (`--session` without `--pipe` and a missing name are rejected), and with `no_pipe_exits_nonzero` in `bin/agent/tests/link.rs`. Check that `deskhop-agent` still has `#![forbid(unsafe_code)]` and passes `cargo archcheck`.
- [x] 3.2 Add the desktop tests in `bin/agent/tests/link.rs`, marked `#[ignore]`, that run the real agent against a test-created named pipe and `ServiceEnd`:
  - `first_report_is_the_screen`;
  - `inject_reaches_the_service_as_injected`: inject F24 down and up, and see the matching `KeyEvent`s marked injected;
  - `service_gone_releases_capture`: set withhold all, drop the service end, and the agent exits within 1 s with code 0.

  Verify that they pass with `cargo test -p deskhop-agent -- --ignored --test-threads=1` on a logged-in desktop.

## 4. Docs and final checks

- [x] 4.1 Fill in the IPC section of `docs/protocol.md`:
  - frames, `Hello` and roles;
  - the registry table with directions;
  - every layout and code table from D2, and the screen limits;
  - the clock exchange;
  - the version-history note that IPC version 1 is unreleased.

  Update `docs/threat-model.md`'s named-pipe row and add an "Agent → service" section:
  - strict decoding and bounded memory;
  - a stalled agent cannot block the service;
  - the link ending releases capture;
  - the open item: impersonating the agent, closed by `add-service`'s random pipe name, ACL and client process ID check.

  Verify that the type numbers in the doc match the registry, using the same check as `add-input-forwarding`.
- [x] 4.2 Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo archcheck`, the Linux test set and the ARM64 check, and run the ignored agent tests on a desktop. Then break one rule at a time and confirm that only the matching tests fail:
  - accept a wrong role;
  - report events before the first `Pong`;
  - keep capture running after the link ends (skip the drop before exit);
  - accept a 0-pixel-wide monitor.

  Restore from copies and touch the files before the final run.
