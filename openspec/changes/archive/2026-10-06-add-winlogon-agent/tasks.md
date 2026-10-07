# Tasks

## 1. A SYSTEM token and the secure desktop (`win32-session`)

- [x] 1.1 Add `system_token(session)` (D2), the `Desktop` enum with a `desktop` parameter on `launch_as` (D3), and a SYSTEM-only agent pipe ACL. Update the crate doc's function table. Verify with unit tests:
  - the agent pipe SDDL for the SYSTEM SID is protected and has exactly one ACE;
  - `system_token` returns an error, not a panic, when run as a normal user;
  - the existing `launch_as` test passes with `Desktop::Default`;
  - `cargo clippy -p win32-session --all-targets -- -D warnings` is clean.

## 2. The agent link per role (`ipc`)

- [x] 2.1 Add `AgentRole` to `ServiceEnd::accept` and `AgentEnd::connect`, and check the peer's role in the handshake (D5). Verify with link tests:
  - both roles link with their matching ends;
  - role `1` on a winlogon pipe is closed (spec "Session agent on a winlogon pipe");
  - role `2` on a session pipe is closed.
- [x] 2.2 Add `InputDesktop` (`0x0019`), `ToService`/`FromAgent::InputDesktop(bool)`, the per-role checks with `ProtocolError::WrongRole`, and refuse `set_clipboard` on a winlogon link (D5). Verify with tests:
  - round trip of both values;
  - value `2` and a 0- or 2-byte payload are protocol errors;
  - `InputDesktop` on a session link closes it;
  - `ClipboardNotShared` on a winlogon link closes it (spec "Clipboard message from a winlogon agent");
  - `set_clipboard` on a winlogon handle returns an error and sends nothing;
  - the never-panic test covers the new type.
- [x] 2.3 Update `docs/protocol.md`:
  - the `InputDesktop` row and code;
  - "session agent → service" on the clipboard rows, and the per-role rule under "Hello and roles";
  - the winlogon agent's command line under "Agent link behavior";
  - the version 1 history note.
  Verify that every row matches the registry in `ipc::message`.

## 3. Reporting the input desktop (`win32-input`)

- [x] 3.1 Add the `report_input_desktop` option to `start`, the 50 ms check on the screen thread, and `Captured::InputDesktop(bool)`, sent once after the first `Screen` and then on each change (D6). Keep the name comparison in a pure function. Verify:
  - unit tests of the comparison, including "input desktop cannot be opened" giving `false`;
  - an ignored desktop test: started on the user's desktop, the reports are `Screen`, then `InputDesktop(true)`.
- [x] 3.2 Turn the option on in `deskhop-agent --trace`, which prints each input desktop change. Verify by hand with the user on this PC: run `--trace`, press Win+L, unlock. The trace shows the input desktop going away and coming back.

## 4. The winlogon agent's mode (`bin/agent`)

- [x] 4.1 Make `parse_mode` accept `--winlogon --pipe <name>` and reject bare `--winlogon` (D7). Verify with the unit tests in `main.rs`.
- [x] 4.2 Turn `session.rs` into `link.rs` with `run(pipe, AgentRole)` (D7):
  - session: clipboard on, desktop reports off;
  - winlogon: clipboard off, desktop reports on, `Captured::InputDesktop` sent as `ToService::InputDesktop`.
  Verify:
  - the existing ignored tests in `bin/agent/tests/link.rs` still pass;
  - new ignored tests `winlogon_reports_screen_then_input_desktop` and `winlogon_ignores_copies` (a PowerShell copy brings no clipboard report within 600 ms);
  - `no_pipe_exits_nonzero` also covers `--winlogon`.
- [x] 4.3 Update the winlogon agent's command line in AGENTS.md (process topology) and in the agent's module doc. Verify that the text matches `parse_mode`.

## 5. Launching and supervising (`bin/service`), then the first secure-desktop check

- [x] 5.1 Change `launch::agent` to `agent(kind, target)`, with the token, pipe ACL, desktop and arguments for each kind (D3). Put the arguments in a pure helper. Verify:
  - a unit test of the arguments for each kind;
  - `processes_start_only_here` still passes;
  - the console-mode dev loop (`cargo run -p deskhop-service -- --console`) still links the session agent.
- [x] 5.2 Add the pure `target(kind, console_mode, console_session, user_logged_on)` and run `supervisor::run(kind)` twice from `main.rs`. `scm.rs` sends `SessionsChanged` to both, stopping waits for both, and in console mode the winlogon task logs once and idles (D4). Verify:
  - a unit test per row of `target`;
  - the existing `Plan` tests;
  - console mode prints the "needs the registered service" line exactly once.
- [x] 5.3 Give `AgentLink` a `kind`, and make `HostCommand::DropLink` take one. In the host, hold the winlogon link beside the session link (D8):
  - send it the capture mode first, and every capture mode after;
  - record and log its `InputDesktop` changes. On becoming inactive, log how many input events it reported while active: a count, no contents;
  - for now, keep injection, warps, input and the screen on the session agent.
  Verify with a host test: a stand-in winlogon agent gets the current mode on linking, and the mode after a focus change.
- [x] 5.4 Manual check with the user on this PC, through the registered service (`tools\dev\service.ps1 install`, then `start`), covering D1's (1), (2), (4) and the token:
  - the winlogon agent runs as SYSTEM in the console session, on the `Winlogon` desktop, at system integrity, with `SeChangeNotifyPrivilege` as its only privilege (Process Explorer, or the commands given at apply time);
  - Win+L, then typing the password, then unlocking: the log shows the secure desktop active, then inactive with a nonzero input count;
  - a UAC prompt (for example, `Start-Process notepad -Verb RunAs`): the log shows active, then inactive;
  - signing out: the log shows the session agent gone and the winlogon agent active. Signing in again brings the session agent back;
  - killing the winlogon agent (from an elevated Process Explorer): a new one links within 2 seconds.

  If any check fails, stop and revisit D1/D2 with the user before group 6.

  Done with the user on 2026-10-06, on Windows 11:
  - the token and the hooks work on the secure desktop;
  - the desktop reports work in both directions;
  - the kill test relinked in 0.5 s.

  Signing out showed that the agent kept launching into the dying session, so the logon screen had no agent for about 30 s. Fixed by reading the target again every second (design D4). Rechecked: the agent linked in the new session 0.1 s after the old one ended, and reported 353 input events at the logon screen.

## 6. Choosing the active agent (`bin/service` host)

- [x] 6.1 Add the `Agents` value with the D8 routing table, as pure code. Verify with unit tests run as sequences of events in and commands out:
  - every row of the table;
  - the spec scenarios "UAC prompt", "Prompt answered", "Winlogon agent lost", "Input from the inactive agent", "Only the winlogon agent at boot", "Session agent restarts during a UAC prompt" and "Agents link at different times";
  - clipboard text never goes to the winlogon agent.
- [x] 6.2 Wire `Agents` into the host in place of 5.3's interim routing. Verify with host tests over loopback, with stand-in agents of both roles:
  - `typing_reaches_the_secure_desktop`: A controls B, and B's winlogon agent reports active, so A's keys reach B's winlogon stand-in and not its session stand-in;
  - `forwarding_survives_a_uac_prompt_on_a`: A forwards to B and A's winlogon agent is active, so physical keys from A's winlogon stand-in reach B;
  - `losing_the_active_winlogon_agent_returns_focus`;
  - `arrival_at_a_locked_machine_sets_clipboard_via_session_agent`;
  - `stop_ends_both_agent_links`.
- [x] 6.3 Update `docs/threat-model.md` (D9): the status line, the "Service → winlogon agent" boundary row, the assets line, and a "Secure desktop (winlogon agent)" section with its residual risks. Verify by reading it against `specs/session-agents`, `specs/secure-desktop` and the code paths in 1.1, 2.2 and 6.1.

## 7. Integration

- [x] 7.1 Run the full checks, and verify that each passes:
  - `cargo fmt --all --check`, `cargo archcheck`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`;
  - the Linux set (`cargo test -p proto -p model -p engine -p policy -p pairing -p ipc`);
  - the ARM64 check from AGENTS.md;
  - the ignored desktop tests in `win32-input`, `deskhop-agent` and `deskhop-service`;
  - a 20-second run of `decode_ipc`.
- [x] 7.2 Manual check with the user on this PC, through the registered service, after 6.2:
  - typing at the lock screen and in a UAC prompt works at this PC's own keyboard;
  - the log shows the active agent switching;
  - the session agent is unaffected after unlocking: the edge and the clipboard log behave as before.
- [ ] 7.3 Manual check with the user on two PCs, deferred with add-onboarding 7.3 and add-text-clipboard 7.3:
  - type into B's UAC prompt from A;
  - unlock B from A;
  - log on to a freshly booted B from A;
  - A shows a UAC prompt while forwarding to B, and typing keeps reaching B;
  - no stuck modifier on B after Win+L from A;
  - repeat the lock and UAC checks on Windows 10 22H2 if a machine is available.

  If a second PC is not available, record that and leave this task open.

  Deferred by the user on 2026-10-06, with add-onboarding 7.3 and add-text-clipboard 7.3, until the rest of the product is complete. On one PC, 5.4 and 7.2 checked:
  - the token;
  - capture on the secure desktop;
  - the desktop reports;
  - following a sign-out to the new session.

  The loopback host tests in 6.2 cover routing between two PCs in-process. Injection into the secure desktop is still unchecked on real hardware.
