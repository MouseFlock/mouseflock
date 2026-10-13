# Design

## Context

The name appears in 154 tracked files, about 760 times, in four kinds of places:

1. **Prose and UI text:** README, docs, ADRs, specs, comments, the app's window and tray strings, installer dialogs.
2. **Build identity:**
   - Cargo packages (`deskhop-service`, `deskhop-agent`, `deskhop-desktop`) and the binaries they produce
   - `tools/archcheck`'s allow-list
   - the npm package
   - `installer/DeskHop.wixproj` and `installer/DeskHopSetup.wixproj`, and the MSI and setup file names
   - `.github/workflows/ci.yml`
   - `apps/desktop/src/useDeskhop.ts`
3. **What lands on a PC:**
   - the service name and install, data and console folders
   - `HKLM\SOFTWARE\DeskHop`
   - the firewall rule names and Start menu shortcut
   - the Tauri identifier `com.deskhop.desktop`, which places WebView2's data folder
   - the autostart Run value the app registers under the name `DeskHop`
   - the pipe names `deskhop-ui` and `deskhop-agent-<hex>`
   - window classes `DeskHopClipboard`, `DeskHopScreen` and `DeskHopFiles`, and thread names
4. **Wire and crypto constants:**
   - `_deskhop._udp.local.` and the `deskhop-<name>.local.` host
   - ALPN `deskhop/1` and `deskhop-pair/1`, and TLS server name `deskhop`
   - the PAKE identity and HKDF salt `deskhop pair v1`
   - TLS exporter label `EXPORTER-deskhop-pair-v1`
   - signature context `deskhop member record v1`
   - file magics `DHID` and `DHTS`
   - the injection tag `0x4448_4f50` (`DHOP`)

No release exists: no tags, version `0.1.0`, and the README has no download. Installs exist only on the project's own test PCs. The user chose a clean break, so all four kinds change.

`add-clipboard-files` is in progress on `feat/clipboard-files`. Its tasks 7.2, 8.1, 8.2 and 8.3 are open.

## Goals / Non-Goals

**Goals:**
- After the change, `git grep -i deskhop` matches only `openspec/changes/archive/` and this change's own artifacts.
- Behavior is identical apart from names. Every existing test passes with renamed fixtures.
- The diff is mechanical and reviewable: file renames, one replacement map, and a short list of hand edits.

**Non-Goals:**
- Migrating identity, pairings or layouts from `C:\ProgramData\DeskHop`. Test PCs pair again.
- Interoperating with DeskHop builds on the wire.
- Dropping the protocol-version-1 and -2 compatibility paths. After the break no such peer can exist, but removing them is a behavior change of its own. It is a follow-up, not part of a rename.
- Renaming the local checkout folder or anything outside the repository.
- Rewriting archived changes.

## Decisions

### D1. Merge `feat/clipboard-files` first, then rename on its own branch

`feat/clipboard-files` merges to `main` as it is, the way earlier features did. The rename then happens on `chore/rename-to-mouseflock`, branched from that `main`. `add-clipboard-files` stays an open change; its four open tasks are listed in the proposal and finished after the rename.

- *Alternative:* rename on the clipboard branch. This mixes a feature and a rename in one review.
- *Alternative:* rename first. The clipboard branch would conflict in most of the files it touches.

### D2. One replacement map, then hand edits for what the map gets wrong

Steps:

1. File renames with `git mv`:
   - `apps/desktop/src/useDeskhop.ts` → `useMouseFlock.ts`
   - `installer/DeskHop.wixproj` → `MouseFlock.wixproj`
   - `installer/DeskHopSetup.wixproj` → `MouseFlockSetup.wixproj`
2. One ordered, case-sensitive replacement over tracked files. It skips `openspec/changes/archive/` (history), this change's own artifacts (which name the old identifiers on purpose) and `openspec/specs/` (main specs change by syncing this change's deltas, plus the hand-edited Purpose sections below):

   | From | To |
   |---|---|
   | `github.com/deskhop/DeskHop` | `github.com/mouseflock/mouseflock` |
   | `DESKHOP` | `MOUSEFLOCK` |
   | `DeskHop` | `MouseFlock` |
   | `Deskhop` | `MouseFlock` |
   | `deskhop` | `mouseflock` |

   This covers prose, package names, pipes, ALPN, mDNS, labels, folders, the registry, the service, class names and test fixtures in one pass. All of them follow the same casing rule today.
3. Hand edits, which the map cannot get right:
   - **File magics:** identity `DHID` → `MFID`, trust store `DHTS` → `MFTS`.
   - **Injection tag:** `0x4448_4f50` (`DHOP`) → `0x4d46_4c4b` (`MFLK`), with its comment.
   - **"DeskHop 0.1.0" as a version-1 peer:** in the clipboard spec, `docs/protocol.md`, `bin/service/src/host/tests.rs` and `crates/transport/tests/clipboard.rs`, this becomes "a peer that speaks only version 1". The map would otherwise produce "MouseFlock 0.1.0", a build that never spoke only version 1.
   - **The design brief's trademark sentence:** dropped (D6).
   - **Two scenario headings:** in `input-capture`, "DeskHop's own injection"; in `clipboard`, "Member running 0.1.0". A MODIFIED delta must keep scenario names, so the deltas keep them and they are renamed in the main specs directly when the change is archived.
   - **The stale version table in `docs/protocol.md`:** it says 2 (speaks 1–2) for both protocols. The code says 3 (speaks 1–3). Corrected while the file is open.
   - **The Purpose sections of the main specs:** a delta cannot change these, so they are edited directly. They are `clipboard`, `desktop-app`, `discovery`, `installer`, `ipc-protocol`, `layout`, `peer-transport`, `secure-desktop`, `service-host`, `ui-link` and `wire-protocol`.
4. `Cargo.lock` is regenerated by `cargo build`. `package-lock.json` comes from `npm install`.

A per-file rewrite by hand was rejected: 760 edits is where mistakes come from. The grep gate in D7 catches anything the map misses.

### D3. No protocol or IPC version bump

The project rule is that a change to `proto` or `ipc` bumps its version constant. That rule exists so that peers, agents and UIs on different builds can negotiate. A clean break leaves nothing to negotiate with:

- a DeskHop peer offers ALPN `deskhop/1`, which a MouseFlock listener refuses during the TLS handshake, before any `Hello`
- a DeskHop UI looks for `\\.\pipe\deskhop-ui`, which no longer exists

Inside the new names, every message, field and semantic is unchanged. So `PROTOCOL_VERSION` and `IPC_VERSION` stay at 3. `docs/protocol.md` gains a short "Names" note: the identifiers changed with the rename, the ALPN is the boundary, and versions continue unchanged.

- *Alternative:* bump both to 4. That adds a version number that means "same as 3" to every future negotiation and to the docs, with no peer on the other side of it. Rejected; the user confirmed this on 2026-10-12.

### D4. Keep the MSI and bundle upgrade codes

`Package.wxs` keeps `UpgradeCode {057CBB71-…}` and `Bundle.wxs` keeps `{E4D0472E-…}`. Installing MouseFlock on a PC that still has DeskHop therefore performs a major upgrade: Windows removes the DeskHop service, programs, firewall rules and shortcut before installing MouseFlock.

With new codes, both products would sit side by side, and two services would compete for UDP 47391. Keeping the codes is the safer failure mode. The major upgrade keeps `C:\ProgramData\DeskHop`, because upgrades keep data. So the clean path for test PCs is still to uninstall DeskHop first (see the migration plan).

Found during apply: the development PC has DeskHop 0.4.0 installed, and MouseFlock builds as 0.1.0. Under the same upgrade code, installing MouseFlock there is a *downgrade*, and the package refuses it with "A newer version of MouseFlock is already installed." The automatic major upgrade only helps against DeskHop builds older than the MouseFlock version. Uninstalling DeskHop first, migration step 3, is therefore required, not just cleaner.

### D5. Wire and crypto identifiers follow the map; the formats do not change

The new values are: `_mouseflock._udp.local.` (10 characters, within DNS-SD's 15-character limit), `mouseflock-<name>.local.`, `mouseflock/1`, `mouseflock-pair/1`, server name `mouseflock`, `mouseflock pair v1`, `EXPORTER-mouseflock-pair-v1`, and `mouseflock member record v1`.

The `v1` suffixes and the ALPN `/1` stay at 1: they version their format, which has not changed, under a new name. Changing the signature context and the magics means a DeskHop trust store or identity file is rejected, not misread. That is acceptable, because the data folder path changes anyway.

### D6. Docs: rename in place, drop the trademark claim

ADRs, `docs/*.md`, AGENTS.md and README describe the current system, so they get the new names without any change to the decisions they record. No new ADR is written; this is a name, not an architectural decision.

The design brief's sentence "The DeskHop name and mark are trademarked" is removed, not converted to "MouseFlock". No mark has been cleared or registered under the new name, and the old claim was doubtful too. ADR 0003 made the same claim. At the user's direction it now reads "will be registered as trademarks", an intent rather than a fact. The decision it records (forks may not use the name) is unchanged. The live `add-clipboard-files` artifacts (4 mentions) are renamed with everything else; archived changes are not.

`docs/threat-model.md` changes names only: the pipes and the service. No trust boundary moves.

### D7. Done means the grep gate plus the full checks

- `git grep -i deskhop -- ':!openspec/changes/archive' ':!openspec/changes/rename-to-mouseflock'` returns only the "Names" note in `docs/protocol.md`, which records the rename.
- `git grep -n -E 'DHID|DHTS|DHOP|0x4448_4f50'` returns nothing outside the archive.
- `cargo archcheck` passes with the renamed allow-list.
- Every check in AGENTS.md passes: build, clippy, tests, the Linux set, the ARM64 check, `npm test`, and the CI `installer` job.

## Risks / Trade-offs

- **A spot the map misses.** For example, a name split across a string concatenation, or `Deskhop` casing in a place nobody grepped. Mitigation: the grep gate is case-insensitive, and the installer test checks service, folder, rule and shortcut names end to end.
- **Leftovers on test PCs from DeskHop installs.** These include:
  - `C:\ProgramData\DeskHop`, holding the old identity's private key seed
  - `%LOCALAPPDATA%\DeskHop\console`
  - an `HKCU\…\Run` value `DeskHop` if "Start with Windows" was ticked
  - the WebView2 folder under `com.deskhop.desktop`
  - a dev-registered `DeskHop` service, which the new package and script no longer recognize

  Mitigation: the migration plan clears each of these on each test PC before installing MouseFlock.
- **No version bump (D3)** departs from a written project rule, by the user's decision. Mitigation: the reason is written in `docs/protocol.md`.
- **`add-clipboard-files` task 8.2's upgrade check** ("upgrade keeps pairing") now has to run MouseFlock to MouseFlock. A DeskHop-to-MouseFlock upgrade drops pairing by design.

## Migration Plan

On each test PC, **before** switching to the renamed code:

1. In the DeskHop app, untick *Start with Windows*, then quit it.
2. If the development service is registered, run the *old* `tools\dev\service.ps1 uninstall` from an elevated shell. The new script looks for `MouseFlock`, not `DeskHop`.
3. If DeskHop is installed, uninstall it from Settings → Apps. This removes `C:\ProgramData\DeskHop` (identity, members, layouts, log).
4. Delete `%LOCALAPPDATA%\DeskHop` if console mode was used.
5. Update the git remote: `git remote set-url origin https://github.com/mouseflock/mouseflock.git`.
6. After building or installing MouseFlock, pair the PCs again.

Rollback: before the merge, delete the branch. After it, `git revert` the rename commit; test PCs then repeat the steps above in reverse.

## Open Questions

- ~~Should D3 bump the versions anyway, to follow the rule literally?~~ Settled 2026-10-12: no bump, documented in `docs/protocol.md`.
- When trademark and domain checks for MouseFlock are done, should the design brief state a mark again? That is outside this change.
