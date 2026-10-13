# Tasks

## 1. Merge clipboard-files and branch (D1)

- [x] 1.1 Record the open `add-clipboard-files` work where it will be seen: add a note under its task 8.2 that the upgrade check now runs MouseFlock to MouseFlock. In the merge's PR description or commit body, list tasks 7.2, 8.1, 8.2 and 8.3 as open, to be finished after `rename-to-mouseflock`.
- [x] 1.2 Merge `feat/clipboard-files` into `main` as it is (a PR, as with earlier features), and pull `main`. Leave `add-clipboard-files` unarchived.
  Done by the user on GitHub before apply (`4a4d93f`, the same tree as the branch tip), without the open-task list in its description; 1.1's note in `add-clipboard-files/tasks.md` and this change's proposal carry that list instead.
- [x] 1.3 Point the remote at the renamed repository: `git remote set-url origin https://github.com/mouseflock/mouseflock.git`. Confirm with `git fetch`.
- [x] 1.4 Create `chore/rename-to-mouseflock` from the updated `main`.

## 2. Prepare the test PCs (user, before building the renamed code)

- [x] 2.1 On each test PC, follow the design's migration plan, steps 1–4:
  - untick *Start with Windows* and quit the app
  - run the **old** `tools\dev\service.ps1 uninstall` if the dev service is registered
  - uninstall DeskHop from Settings → Apps
  - delete `%LOCALAPPDATA%\DeskHop` if console mode was used

  Confirm no `DeskHop` service remains: `Get-Service DeskHop` fails.

  Done by the user on the test PCs (2026-10-13).

## 3. Mechanical rename (D2)

- [x] 3.1 `git mv` the three files: `apps/desktop/src/useDeskhop.ts` → `useMouseFlock.ts`, `installer/DeskHop.wixproj` → `MouseFlock.wixproj`, `installer/DeskHopSetup.wixproj` → `MouseFlockSetup.wixproj`.
- [x] 3.2 Apply D2's replacement map, in order, to every tracked file except `openspec/changes/archive/`, `openspec/changes/rename-to-mouseflock/` and `openspec/specs/`. Preserve each file's line endings, since the repo mixes CRLF and LF.
- [x] 3.3 Regenerate `Cargo.lock` with `cargo build --workspace`, and `apps/desktop/package-lock.json` with `npm install` in `apps/desktop`. Confirm only the package names changed in both.

## 4. Hand edits (D2, D3, D5, D6)

- [x] 4.1 File magics: in `crates/pairing`, change identity `DHID` → `MFID` and trust store `DHTS` → `MFTS`, along with any test fixtures that spell them.
- [x] 4.2 Injection tag: in `crates/win32-input/src/decide.rs`, change `TAG` from `0x4448_4f50` to `0x4d46_4c4b` and update its comment ("DHOP" → "MFLK").
- [x] 4.3 Reword "MouseFlock 0.1.0" (left by the map where it meant "DeskHop 0.1.0") to "a peer that speaks only version 1". Places: `bin/service/src/host/tests.rs`, `crates/transport/tests/clipboard.rs`, `docs/protocol.md`, and any other match of `git grep -n "MouseFlock 0.1.0"`.
- [x] 4.4 `docs/protocol.md`:
  - correct the version table to `proto::PROTOCOL_VERSION` 3 (speaks 1–3) and `ipc::IPC_VERSION` 3 (speaks 1–3), as the code says
  - add a "Names" note: identifiers renamed from DeskHop on 2026-10-12, the ALPN and pipe names are the boundary, messages and versions unchanged (D3)
- [x] 4.5 `docs/design-brief.md`: remove the trademark sentence (D6). Check that the rest of the "Open source" paragraph still reads well. ADR 0003's claim now reads "will be registered as trademarks" (user decision, 2026-10-12).
- [x] 4.6 Edit the Purpose sections of `clipboard`, `desktop-app`, `discovery`, `installer`, `ipc-protocol`, `layout`, `peer-transport`, `secure-desktop`, `service-host`, `ui-link` and `wire-protocol` in `openspec/specs/` directly, then sync this change's delta specs into the main specs (`/opsx:sync`).
- [x] 4.7 Confirm `docs/threat-model.md` changed names only (pipes, service), with no change to any boundary or mitigation.

## 5. Verify (D7)

- [x] 5.1 Grep gates. `git grep -n -E 'DHID|DHTS|DHOP|0x4448_4f50' -- ':!openspec/changes/archive' ':!openspec/changes/rename-to-mouseflock'` returns nothing. `git grep -i deskhop -- ':!openspec/changes/archive' ':!openspec/changes/rename-to-mouseflock'` returns only the "Names" note at the top of `docs/protocol.md`, which records the old name on purpose, and the scenario heading "DeskHop's own injection" in `openspec/specs/input-capture/spec.md`, which 6.2 renames.
- [x] 5.2 Run and pass:
  - `cargo fmt --all --check`, `cargo archcheck`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
  - the Linux set
  - the ARM64 check, with the renamed packages
  - `npm test` in `apps/desktop`
  - `openspec validate rename-to-mouseflock`

  Passed (2026-10-12): fmt, archcheck (15 packages), clippy, `cargo test --workspace` (46 suites), the Linux set (23 suites), the ARM64 check, `npm test` (21), `openspec validate --strict`. The first run had three failures, all expected from the rename:
  - rustfmt reflowed lines that grew past the width limit.
  - `pairing::confirm::tests::known_answer` failed because its tags derive from the renamed labels. Both tags were recaptured from the implementation, as the test's comment says they were made.
  - Two log-privacy fixtures in `bin/service/src/host/tests.rs` changed length. They were rewritten to their original lengths: the file body is 26 bytes again, and the image marker fits its 24-byte slot.

  Not run: the `--ignored` agent, service and clipboard desktop tests. They take over this PC's clipboard and pipes while the installed DeskHop is still running.
- [x] 5.3 `installer\build.ps1 -Arch x64` produces `MouseFlock-<version>-x64.msi` and `MouseFlock-Setup-<version>-x64.exe`. Then the user runs `installer\test.ps1` against them in an elevated shell. It checks the `MouseFlock` service, `C:\Program Files\MouseFlock`, the `MouseFlock (UDP 47391)` and `MouseFlock (mDNS)` rules, the Start menu shortcut and the Installed apps entry. The CI `installer` job passes with the renamed globs.

  Passed: `installer\test.ps1` on this PC, run by the user (2026-10-13). The CI `installer` job passed on #40.
- [x] 5.4 Manual check with the user on two PCs, both installed fresh:
  - the PCs find each other over `_mouseflock._udp`, pair with a code, and the cursor crosses
  - typing works, and copied text and an image paste on the other PC
  - the tray says "MouseFlock: …"
  - `C:\ProgramData\MouseFlock` holds the new identity

  Passed on two PCs with fresh installs, checked by the user (2026-10-13).

## 6. Land it

- [x] 6.1 Commit as `chore(repo): rename DeskHop to MouseFlock`. Open the PR from `chore/rename-to-mouseflock`, noting the agent and model used and listing the migration steps for anyone with a DeskHop install. Opened as https://github.com/mouseflock/mouseflock/pull/40.
- [x] 6.2 After merge, archive this change. In the same archive commit, rename two scenario headings in the main specs directly; a MODIFIED delta cannot rename a scenario. In `input-capture`, "DeskHop's own injection" becomes "MouseFlock's own injection". In `clipboard`, "Member running 0.1.0" becomes "Member that speaks only version 1". Re-run the 5.1 grep gate: only the `docs/protocol.md` "Names" note remains.
- [ ] 6.3 Resume `add-clipboard-files` at tasks 7.2 and 8.1–8.3.
