# Tasks

## 1. ADR 0006 and `win32-session`

- [x] 1.1 Write `docs/adr/0006-win32-session.md`, status Proposed: a third `unsafe` crate linked only by `bin/service`, with alternatives and consequences (D1). Update:
  - AGENTS.md: repo layout, dependency rules 2 and 3, the Win32 rule on launches, and the ARM64 command;
  - `docs/design-brief.md`: "three crates".
  Verify by reading that each document names `win32-session` consistently.
- [x] 1.2 Create `crates/win32-session` (`unsafe` allowed, every block with `// SAFETY:`) with the `windows` features it needs. Extend `tools/archcheck`:
  - `win32-session` joins the crates allowed `unsafe`;
  - only `deskhop-service` may depend on it.
  Add `-p win32-session -p deskhop-service` to CI's ARM64 check. Verify that `cargo archcheck` passes, and that a scratch dependency from `deskhop-agent` on `win32-session` makes it fail; revert the scratch change afterwards.
- [x] 1.3 Add `create_agent_pipe`, `client_process_id` and `own_sid` (D1). Verify with tests run as a normal user:
  - `pipe_for_own_user_opens`, where the client process ID equals `std::process::id()`;
  - `pipe_for_other_sid_denies`, with Guests and access denied;
  - `second_instance_fails`;
  - SDDL construction unit tests.
- [x] 1.4 Add `active_console_session`, `user_token`, `Token::user_sid`, `launch_as` and `Process::{id, wait, terminate}`. Verify that a unit test of `active_console_session` returns this desktop's session. `user_token` returns an error rather than panicking when not SYSTEM. The SYSTEM paths are checked in 8.3.
- [x] 1.5 Add `secure_folder`, `sas_policy_allows_services` and `send_sas`, with `SendSAS` declared by `#[link]` if the `windows` crate lacks it (D1, D7, D10). Verify:
  - the owner-classification unit tests (SYSTEM and Administrators trusted, a user SID not);
  - `sas_policy_allows_services` reads this machine's value without error;
  - the SYSTEM paths are checked in 8.3.

## 2. Engine: losing local input

- [x] 2.1 Add `engine::Event::LocalInputLost` (D4) and document it in the crate docs. Verify with replay tests named after the spec scenarios, which also run in the Linux set:
  - `agent_ends_while_forwarding`: Ctrl held; `FocusWithdrawn` to B; `SetCapture(PassAll)`; focus is local;
  - `agent_ends_while_controlled`: `TakenBack` to A; releases for injected keys; focus is local;
  - `agent_ends_with_local_focus`: nothing sent;
  - `release_after_relink_is_dropped`: a physical key held before the loss, whose release arrives on the new link, is not forwarded.

## 3. Store, identity and logs in `bin/service`

- [x] 3.1 Add the `bin/service` dependencies (`engine`, `model`, `win32-session`, `windows-service`, `tokio`, `serde`, `serde_json`, `rand_core`). Add `log.rs` (D12): sink, levels, UTC timestamps, 1 MiB rotation. Verify with unit tests: the timestamp format for known Unix times, including a leap day, and rotation keeping exactly one `.old` file.
- [x] 3.2 Add `store.rs` (D7), covering:
  - data folder selection (service, console default, `--data`);
  - `secure_folder` in service mode only;
  - the four file formats with the 1 MiB read cap;
  - atomic replace;
  - quarantine to `.bad-<secs>`;
  - the store task, which writes only the newest contents.
  Verify with unit tests:
  - each format round-trips, the `layouts.json` example from D7 included;
  - an unknown `version`, bad hex, bad JSON and an oversized file are quarantined, and the store loads as absent;
  - a `.tmp` left behind does not replace the real file;
  - 100 updates queued in a burst end with the last contents on disk.
- [x] 3.3 Add identity loading and creation, and the machine name (D8). Verify with unit tests:
  - a first load creates and stores an identity, and a second load returns the same peer ID;
  - a damaged `identity.key` is quarantined and replaced, with an error logged;
  - a 70-byte multi-byte `COMPUTERNAME` is cut at a character boundary to at most 63 bytes;
  - an empty name gives `"Windows PC"`.

## 4. Host loop

- [x] 4.1 Add `host.rs` (D3): the `select!` loop, the event and decision tables, the `Desk` mirror, the set of up peers, the service clock, `LayoutsLoaded` before the first link, capture mode first on every new link, and `LocalInputLost` when a link ends. Add a test harness: two hosts on loopback with discovery off, temp data folders, and fake agents (`AgentEnd` over duplex). Verify with host tests:
  - `pairing_through_commands`;
  - `cursor_crosses`: A learns B by pushing; A's agent gets `SetCapture(WithholdAll)`; B's agent gets `Warp` then `Inject` motion;
  - `new_link_gets_capture_mode_first`: B is controlled by A while it has no agent; the agent that then links gets `SetCapture(WithholdMouse)` first. With local focus, a new link gets `SetCapture(PassAll)` first;
  - `link_lost_while_forwarding_returns_focus`;
  - `peer_down_while_controlled_passes_all`;
  - `restart_keeps_desk_and_layout`.
- [x] 4.2 Add `HostCommand::{Suspend, Resume, Stop}` handling, with stop ordered as goodbye, then drop the link, then drain the store, then shut the node down (D9). Verify with host tests:
  - `suspend_says_goodbye`: B sees `Goodbye` and its agent passes all;
  - `resume_reconnects`;
  - `stop_while_forwarding`: B sees `Goodbye` and releases A's keys; A's fake agent sees its link end; the store has drained.
- [x] 4.3 Keep input out of the log. Verify with host test `log_has_no_input`: with a capturing sink, type 20 keys while forwarding; no logged line contains any of their usage values or the word `Inject`.

## 5. Supervisor and launching

- [x] 5.1 Add the pure `Plan` (D5). Verify with unit tests on fake time:
  - logon launches; logoff ends; a console switch ends, then launches in the new session;
  - no user means no launch;
  - exits back off 0.5, 1, 2, 4, 8, 16, 30, 30 s;
  - the backoff resets after 30 s linked;
  - a 10 s start deadline terminates;
  - a 2 s end deadline terminates;
  - stop ends the agent and launches nothing more.
- [x] 5.2 Add `launch::agent` (D6), the single launch function, with its `Session` and `Console` targets. Add the driver:
  - pipe names from 16 random bytes, retried on a name clash;
  - connect plus handshake within 10 s;
  - the process ID check before `ServiceEnd::accept`;
  - the `AgentLink` with `ended`;
  - session re-evaluation.
  Verify with tests:
  - `pid_mismatch_is_closed_unread`: an agent pipe and an expected ID that is not ours; the test's client is closed and nothing reaches a host;
  - `pipe_names_are_unique` over 1,000 names;
  - a grep showing that no `Command::new`, `CreateProcess` or `launch_as` call exists outside `launch.rs`.

## 6. Console mode

- [x] 6.1 Parse `--console [--data <dir>] [--port <n>]` (D2, D11). Exit nonzero with the port named when binding fails. Verify with unit tests: argument errors, and a bound UDP socket's port making console start fail with that port in the message.
- [x] 6.2 Add the console commands and output (D11), including Ctrl+C. Verify with unit tests for parsing and peer resolution (exact name, case, hex prefix, ambiguous prefix, a one-character prefix rejected), and with host test `console_prints_learned_side_with_undo_hint`.
- [x] 6.3 Add the desktop tests in `bin/service/tests/console.rs`: `#[ignore]`, with kill-on-drop guards for the service and every agent it starts.
  - `console_links_its_agent` (`--data <tmp> --port 0`);
  - `quit_ends_service_and_agent`, where both exit 0;
  - `killed_agent_is_replaced`: a new agent is linked within 2 s.
  Verify that they pass with `cargo test -p deskhop-service -- --ignored --test-threads=1`, and that no `deskhop-agent` process remains afterwards.

## 7. Windows service

- [x] 7.1 Add `scm.rs` (D9), including the dispatcher entry, moved into `win32-session` if the macro trips `forbid(unsafe_code)`. Covers status reporting, the accepted controls, session changes to the supervisor, suspend with a 1 s bounded reply, resume, and stop and shutdown. Add the SAS path (D10). Verify that `deskhop-service.exe` run from a terminal with no arguments exits at once with the SCM's "not started by the service manager" error, logged. Also verify with a unit test of the mapping from control to `HostCommand`.
- [x] 7.2 Write `tools/dev/service.ps1` (D13): `install`, `uninstall`, `start`, `stop`, `status`, `log`. It refuses to run when not elevated and prints every system change it makes. Verify that, run from a non-elevated shell, it prints the elevation message and changes nothing. Document its use in AGENTS.md's Commands.

## 8. Threat model and integration

- [x] 8.1 Update `docs/threat-model.md`. Close the "Agent → service" open item, describing the random pipe name, the single instance, the ACL and the process check. Then add:
  - the data folder: owner, ACL and the move-aside rule;
  - console mode, and why it keeps its own folder;
  - secure attention and its policy;
  - the residual risks from the design;
  - the status line.
  Verify by reading it against `specs/session-agents` and `specs/service-host`.
- [x] 8.2 Run the full checks:
  - `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` and `cargo archcheck`;
  - the Linux set from AGENTS.md (left to CI if the target is missing locally);
  - the ARM64 check;
  - the ignored desktop tests in `win32-input`, `deskhop-agent` and `deskhop-service`.
  Then run mutation checks, each restored afterwards, and confirm that each fails only its matching tests:
  - skip the process ID check;
  - send the capture mode after other commands on a new link;
  - drop `LocalInputLost`;
  - write files in place without the temporary file (caught by `replacing_never_writes_into_the_old_file`, added for this check).
- [x] 8.3 Manual check with the user on this PC. The user runs `tools/dev/service.ps1 install` and `start` from an elevated shell, and confirms:
  - the agent runs as the user at medium integrity (Task Manager details);
  - killing it brings a new one within 2 s;
  - `icacls C:\ProgramData\DeskHop` shows only SYSTEM;
  - the pipe denies another user;
  - sleep and resume are logged;
  - Ctrl+Alt+End from a peer shows Ctrl-Alt-Del (or, with one PC, that the policy check logs correctly);
  - `uninstall` removes everything but the data folder.

  Done 2026-10-05 on Windows 11 (BOSGAME-G28LW):
  - **Agent account:** `BOSGAME-G28LW\sharath`, Console session. Its token is Limited, Medium integrity (0x2000), with Administrators deny-only, the same as Explorer's.
  - **Restart:** a killed agent was relaunched 0.5 s later and linked at 0.55 s.
  - **Folder:** `icacls` from an elevated shell got "Access is denied" (SYSTEM-only), yet `service.ps1 log` read `service.log` (admin-readable log ACL).
  - **Sleep:** two sleep and resume cycles were logged as goodbye and reconnect. Windows sends two resume events per wake, and both are handled.
  - **Uninstall:** removed the service, both firewall rules and the Program Files copy, and left the data folder and the SAS policy as designed.
  - **Pipe denying another user:** not checked by hand (no second account); covered by `pipe_for_other_sid_denies`.
  - **Ctrl+Alt+End:** not checkable with one PC. The registered service cannot pair before the UI exists, so the end-to-end `SendSAS` check moves to `add-onboarding`; the console-mode case is in 8.4.
- [ ] 8.4 Manual check with the user on two PCs in console mode:
  - pair with `pair`/`join`;
  - push across and confirm that layout learning prints the side with the Undo hint;
  - type and click on the other PC, then cross back;
  - take control back locally;
  - Ctrl+Alt+End on A while it controls B: B logs that secure attention needs the registered service, and stays controlled;
  - restart both and cross again without pairing or learning.
  If a second PC is not available, record that and leave this task open.

  Deferred by the user on 2026-10-05: no second PC was available. Carry this check, with the end-to-end Ctrl+Alt+End check, into the next change that runs on two PCs. The same path (pair through commands, learn by pushing, cross, inject, cross back, restart and cross again) is covered in-process by the host tests over loopback.
