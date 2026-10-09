# Tasks

## 1. Verifying the approach (decision changed: direct, not a throwaway spike)

The user chose on 2026-10-08 to verify the real product code directly with a 0.3.0 build (6.2) rather than a separate throwaway spike, since the product code is small and unit-tested. Tasks 1.1–1.4 below are superseded; the verification lives in 6.2, and the shield tamper-check in a committed probe script.

- [x] 1.1 Superseded by direct verification (see 6.2). Instead of a throwaway program,  runs the shield tamper-check against the real agent.
- [x] 1.2 Superseded: the run sheet is folded into 6.2.
- [x] 1.3 Superseded: the approach is confirmed by 6.2 on real hardware.
- [x] 1.4 Superseded: no separate spike to feed back from.

## 2. The UI-access token and a shielded launch (`win32-session`)

- [x] 2.1 Add `Token::ui_access()` and `Token::integrity()` (D5). Verify with unit tests on the test process's own token: no UI access, and integrity medium (`0x2000`) or high (`0x3000`). Every new `unsafe` block has a `// SAFETY:` comment.
- [x] 2.2 Add the session agent's token builder (D2): user token, duplicated as primary, raised to Medium Plus, then UI access. It returns which step failed. Verify:
  - a unit test where, run as a normal user, it returns an error naming the step, not a panic;
  - `cargo clippy -p win32-session --all-targets -- -D warnings` is clean;
  - the crate doc's function table lists it.
- [x] 2.3 Add an optional process and thread security descriptor to `launch_as` (D3): SYSTEM full access; the user `SYNCHRONIZE`, `PROCESS_QUERY_LIMITED_INFORMATION` and `PROCESS_TERMINATE`; `OWNER RIGHTS` with nothing more; a Medium Plus no-write-up label. Keep the SDDL in a pure function. Verify:
  - a unit test of the SDDL for a sample SID: protected, with exactly those ACEs and that label;
  - the existing `launch_as` test passes without a descriptor;
  - clippy is clean.

## 3. Launching with UI access (`bin/service`)

- [x] 3.1 In `launch::agent`, launch the session agent in a session with the UI-access token and the descriptor. On any failure, launch with the plain token and no descriptor, and return the reason (D4). `Watch` carries whether UI access was given. Verify:
  - unit tests: console mode reports no UI access; the reason text names the failed step;
  - `processes_start_only_here` still passes;
  - every launch still goes through `launch::agent`.
- [x] 3.2 In the supervisor, add `UI access: yes|no` to the session agent's launch log line. Log the fallback reason once per service run, and in console mode log once that elevated windows need the registered service (D4). Verify:
  - in console mode on this PC (`cargo run -p deskhop-service -- --console`), the log shows the console line once and the launch line with `UI access: no`;
  - a unit test that the log-once helper logs the first reason and not a repeat.

## 4. Setting changes at the raised level (`win32-input`, only if spike run C needs it)

- [x] 4.1 If run C showed `WM_SETTINGCHANGE` dropped, allow that one message through on the screen thread's window with `ChangeWindowMessageFilterEx` (D3). Verify:
  - an ignored desktop test that the filter call succeeds on a window the thread creates;
  - in the 6.2 check, changing scaling in Settings gives a new screen report.

  Done: added proactively rather than gated on a spike (the user chose direct verification). At Medium Plus, UIPI drops a WM_SETTINGCHANGE that an unelevated process broadcasts, so the screen window allows that one message with ChangeWindowMessageFilterEx; it is a no-op at medium integrity. The scaling-change check is in 6.2.

## 5. Docs

- [x] 5.1 Write `docs/adr/0008-session-agent-ui-access.md`, status Proposed, refining ADR 0002 as D7 says. Verify by reading it against D2, D3 and ADR 0002's table.
- [x] 5.2 Add an "Elevated windows (UI access)" section to `docs/threat-model.md`. It covers how UI access is granted, the shield, what a member can now do, the fallback and the residual risks. Remove the "Elevated windows" residual risk from the secure desktop section, and update the status line. Verify by reading it against `specs/session-agents` and D2–D4.
- [x] 5.3 Update AGENTS.md's process topology line for the session agent ("as the logged-on user, with UI access"), and `docs/known-issues.md`'s admin-window entry once the trap is fixed. Verify that they match ADR 0008 and the 6.2 result.

  Done: AGENTS.md updated; after 6.2 confirmed the fix, docs/known-issues.md marks the admin-window trap fixed (UI access, ADR 0008), keeping the hidden-screen entry.

## 6. Integration

- [ ] 6.1 Run the full checks, and verify that each passes:
  - `cargo fmt --all --check`, `cargo archcheck`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`;
  - the Linux set. `engine` is untouched, so its replay tests run unchanged;
  - the ARM64 check;
  - `npm test`;
  - the CI run, including the `installer` job.
- [x] 6.2 Manual check with the user on both PCs. Build with `installer\build.ps1 -Arch x64 -Version 0.3.0`, and install over the current version with the setup program on both, with the PCs paired:
  - **Upgrade:** both PCs report each other up again without pairing (add-installer 6.3, upgrade part). The service log shows `UI access: yes`.
  - **The trap, now passing:** controlling one PC from the other, an elevated Notepad on the controlled PC accepts typing, clicks and motion, and the cursor crosses back. The same with Task Manager in front.
  - **At a PC's own keyboard:** with an elevated window in front, the cursor crosses out to the other PC. While controlled with an elevated window in front, the PC's own mouse or keyboard takes control back.
  - **The shield:** run `toolsdevprobe-agent-shield.ps1` unelevated: it reports every tampering right denied and only terminate allowed. Ending the agent in Task Manager relinks within 2 seconds, with UI access.
  - **Everything else still works:** the clipboard both ways, text and images; changing scaling in Settings gives a new screen report; Win+L and a UAC prompt still switch to the winlogon agent.
  - **Security software:** Windows Security's protection history shows nothing for DeskHop.

  Done with the user on 2026-10-09, on the 0.3.0 build over 0.1.0:
  - the service log shows `launched session agent 127860 in session 3, UI access: yes`, with no fallback warning;
  - an admin Notepad and Task Manager on the controlled PC take typing, clicks and motion (the trap is gone);
  - `toolsdevprobe-agent-shield.ps1` against the session agent (which runs as the user) reports VM write, create-thread and WRITE_DAC all denied, and only terminate allowed: the shield holds;
  - clipboard and the secure desktop (Win+L, UAC) still work in the log.
  - The probe script was fixed during the check to select the session agent by owner, since the winlogon agent (SYSTEM) was picked first and its command line is unreadable across the integrity boundary.
- [ ] 6.3 Repeat 6.2's trap and shield checks on Windows 10 22H2, deferred with the other two-PC checks if no machine is available. If none is available, record that and leave this task open.
