# Proposal

## Why

The name DeskHop is already taken in this exact category. `hrvach/deskhop` is a hardware KVM ("Fast Desktop Switching Device", about 7,900 stars, public since December 2023). It splits search results with this project, and it predates the trademark claim in `docs/design-brief.md`. The GitHub organization and repository are already renamed to `mouseflock/mouseflock`. The rest of the repository still says DeskHop. Nothing has been released publicly, so this is the cheapest time to finish the rename.

## What Changes

- **Product name:** DeskHop becomes MouseFlock everywhere a user reads it: the window and tray text, the Start menu shortcut, Installed apps, the setup program, firewall rule names and the README.
- **BREAKING: binaries and packages.** `deskhop-service.exe`, `deskhop-agent.exe` and `DeskHop.exe` become `mouseflock-service.exe`, `mouseflock-agent.exe` and `MouseFlock.exe`. The Cargo packages `deskhop-service`, `deskhop-agent` and `deskhop-desktop` become `mouseflock-service`, `mouseflock-agent` and `mouseflock-desktop`. The npm package is renamed to match. The installer outputs become `MouseFlock-<version>-<arch>.msi` and `MouseFlock-Setup-<version>-<arch>.exe`.
- **BREAKING: what lands on a PC.** The `DeskHop` service becomes `MouseFlock`. Folders change:
  - install folder: `C:\Program Files\MouseFlock`
  - data folder: `C:\ProgramData\MouseFlock`
  - console-mode data: `%LOCALAPPDATA%\MouseFlock\console`
  - development install: `Program Files\MouseFlock Dev`

  The registry key becomes `HKLM\SOFTWARE\MouseFlock`, and the Tauri identifier becomes `com.mouseflock.desktop`.
- **BREAKING: local pipes.** `\\.\pipe\deskhop-ui` and `\\.\pipe\deskhop-agent-<hex>` become `\\.\pipe\mouseflock-ui` and `\\.\pipe\mouseflock-agent-<hex>`.
- **BREAKING: wire and crypto identifiers (a clean break).**
  - mDNS service type: `_mouseflock._udp.local.`
  - ALPN values: `mouseflock/1` and `mouseflock-pair/1`
  - TLS server name: `mouseflock`
  - pairing labels: `mouseflock pair v1` and `EXPORTER-mouseflock-pair-v1`
  - member-record signature context: `mouseflock member record v1`
  - file magics: identity `MFID`, trust store `MFTS`
  - injection tag: `MFLK`

  A MouseFlock build cannot see or talk to a DeskHop build, and pairings made under DeskHop do not carry over. Each test PC uninstalls DeskHop and pairs again.
- **Documentation and specs:** README, AGENTS.md, the design brief, the ADRs, `docs/protocol.md`, `docs/threat-model.md`, `docs/known-issues.md`, the live specs and `openspec/config.yaml` use the new name. The design brief's trademark sentence is dropped until a clearance search is done; it currently claims a mark the project does not hold.
- **Not changed:**
  - the archived changes under `openspec/changes/archive/`, which record what was decided at the time
  - UDP port 47391
  - protocol and IPC version numbers (see design D3)
  - the MSI and bundle upgrade codes (see design D4)
  - any behavior beyond the names

### Sequencing, and the unfinished clipboard-files work

The in-progress change `add-clipboard-files` is merged to `main` as it is, before the rename starts, so the rename is one mechanical diff on top of a single branch. The change stays open and is finished after the rename. Its four unfinished tasks are:

- **7.2:** update `docs/known-issues.md` only if the two-PC check in 8.2 finds a limitation users will hit.
- **8.1:** the full checks: fmt, archcheck, clippy, tests, the Linux set, the ARM64 check, `npm test`, and the CI run including the `installer` job.
- **8.2:** the manual two-PC check on a build installed as an upgrade, which also covers add-installer 6.3 (an upgrade keeps pairing). After the rename, this upgrade is MouseFlock to MouseFlock: two MouseFlock builds of different versions. An upgrade from DeskHop no longer keeps pairing, by design.
- **8.3:** 8.2's single-file and folder checks on Windows 10 22H2.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `agent-link`: the agent's executable name on its command line.
- `desktop-app`: the app's executable name, the UI pipe name, and the product name in what the app says.
- `discovery`: the mDNS service type that machines advertise and browse.
- `peer-transport`: the ALPN values for member and pairing connections.
- `desk-membership`: the signature context for membership records, and the trust store file magic.
- `pairing`: the identity file magic.
- `service-host`: the service name and the service executable.
- `ui-link`: the UI pipe name.
- `installer`: the shipped file names, install and data folders, service name, Installed apps entry, Start menu shortcut and firewall rules.
- `clipboard`: one scenario that names DeskHop 0.1.0 as the version-1 peer.
- `input-capture` and `input-focus`: the product name in requirement text. Behavior is unchanged.

## Impact

- **Crates:**
  - `proto`: signature context
  - `pairing`: labels and file magics
  - `transport`: mDNS type, host name, ALPN, server name
  - `ipc`: UI pipe name
  - `model`: a doc comment
  - `win32-input`: injection tag, window class, thread names
  - `win32-clipboard`: window class names, thread names, test strings
  - `win32-session`: test paths
  - `win32-shell`: a doc comment and test pipe names
  - `bin/service`, `bin/agent`, `apps/desktop` (src-tauri and React), `tools/archcheck`

  No dependency is added or removed. The dependency rules in AGENTS.md hold unchanged: only the package names in archcheck's allow-list change, not which crate may link what.
- **Build and CI:** `Cargo.toml` (repository URL), `Cargo.lock`, `.github/workflows/ci.yml`, `installer/` (renamed `.wixproj` files and scripts), and the `tools/` scripts.
- **Developer machines:** before pulling the rename, run the old `tools\dev\service.ps1 uninstall` and uninstall DeskHop from Settings. Neither the new script nor the new package recognizes a service named `DeskHop`. Update the git remote to `https://github.com/mouseflock/mouseflock.git`.
- **Outside the repo, for the user to decide:** the local checkout folder `C:\projects\DeskHop`, and trademark and domain checks for MouseFlock.
