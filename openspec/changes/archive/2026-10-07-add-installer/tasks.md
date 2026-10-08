# Tasks

## 1. Removing the data folder safely (`win32-session`, `bin/service`)

- [x] 1.1 Add `win32_session::remove_secure_folder(path)`, returning `Removed`, `Absent` or `Refused`, built on `folder.rs`'s owner and reparse checks (D4). Verify with unit tests:
  - an absent path gives `Absent`;
  - a junction to a temp folder (made with `cmd /c mklink /J`) gives `Refused`, and both the junction and the file in its target remain;
  - a folder this user owns gives `Refused` and remains;
  - clippy is clean, and every new `unsafe` block has a `// SAFETY:` comment.
- [x] 1.2 Add `deskhop-service --remove-data` (D4): it removes `store::service_dir()`, prints the outcome, and exits 0. Verify:
  - `parse_args` unit tests for the new mode and for its misuse (`--remove-data --console`);
  - run as a normal user on a PC with no data folder, it prints that there is nothing to remove; here the dev service's SYSTEM-only folder exists, so a normal user gets "could not remove … Access is denied" and exit 0, and the absent case is the unit test `removing_an_absent_folder`;
  - `processes_start_only_here` still passes.

## 2. The package (`installer/`)

- [x] 2.1 Create `installer/DeskHop.wixproj`, pinned to `WixToolset.Sdk` 5.0.2 and its `Util`, `Firewall` and `UI` extensions, and `Package.wxs` with the package, the major upgrade, the files and the Start menu shortcut (D1, D3). Add `installer/obj/` and `installer/bin/` to `.gitignore`. Verify that `dotnet build` produces an MSI with stand-in files, and that WiX's ICE validation reports no errors.
- [x] 2.2 Add the service registration, failure actions and service control (D3). Verify by inspecting the built MSI's `ServiceInstall`, `ServiceControl` and `MsiServiceConfigFailureActions` tables (the util extension keeps failure actions in its own `Wix4ServiceConfig` table) (`wix msi decompile`, or Orca-style `Get-MsiTable` in `test.ps1`).
- [x] 2.3 Add the two firewall exceptions (D3), the SAS policy component with its searches (D5), and the `HKLM\SOFTWARE\DeskHop` markers. Verify the tables as in 2.2.
- [x] 2.4 Add the WebView2 search and the bootstrapper custom action (D6), the data-removal custom action (D4), `CloseApplication` for `DeskHop.exe` (D3), and the dev-service launch condition (D8). Verify the custom action conditions and their sequencing in the decompiled MSI against D4, D6 and D8.
- [x] 2.5 Add the UI: progress, then the finish page with "Start DeskHop" checked by default, plus the files-in-use and error dialogs (D7). Verify the dialog set and the checkbox's `DoAction` in the decompiled MSI.

## 3. Building (`installer/build.ps1`)

- [x] 3.1 Write `installer/build.ps1 -Arch x64|arm64` (D2):
  - the version from `cargo metadata`;
  - `tauri build --no-bundle`, then the release service and agent for the target;
  - the WebView2 bootstrapper, downloaded once, with its signature checked;
  - `dotnet build`;
  - for ARM64, a check for the MSVC ARM64 libraries with a message saying how to install them.

  Verify:
  - `installer\build.ps1 -Arch x64` produces `installer\out\DeskHop-0.1.0-x64.msi`;
  - the `DeskHop.exe` inside is a release build: it loads its bundled frontend, and with no service in session 0 it shows that the service isn't running;
  - a tampered bootstrapper (its bytes changed) stops the build.

  Done: the x64 build produced the MSI; the genuine bootstrapper checked as signed by Microsoft Corporation; a copy with one byte flipped stopped the build (HashMismatch) and left the MSI unchanged. The installed release `DeskHop.exe` is checked in 6.2 instead of here: a release build's first launch registers itself in the user's Run key, which would point at `target`.
- [x] 3.2 Try `installer\build.ps1 -Arch arm64` here. If the ARM64 libraries are missing, verify that the message says how to install them, and leave the ARM64 build to CI (4.2).

- [x] 3.3 Add the setup program (D11), added after the 6.2 check found that an MSI run with a UI warns of a reboot when DeskHop is running:
  - `DeskHopSetup.wixproj` and `Bundle.wxs`;
  - `build.ps1` builds it after the MSI;
  - remove the MSI's custom Installed apps entry and `ARPSYSTEMCOMPONENT` from the earlier attempt, so the bundle owns the entry, and the MSI keeps its own when installed directly.

  Verify:
  - `build.ps1` produces `DeskHop-Setup-0.1.0-x64.exe` and `DeskHop-Setup-0.1.0-arm64.exe`;
  - the MSI's tables no longer hold the custom entry;
  - the bundle's manifest chains the MSI with `Visible="no"`, `LaunchTarget` and the dev-service condition.

## 4. Checking (`installer/test.ps1`, CI)

- [x] 4.1 Write `installer/test.ps1 -Msi <path> -UpgradeMsi <path> [-Setup <exe>]` with the install, upgrade and uninstall checks from D9, plus the setup program's install and uninstall. Verify with the user, in an elevated PowerShell on this PC with the dev service uninstalled, that it passes against the x64 MSIs and setup program.
- [x] 4.2 Add the CI `installer` job (D9): it builds both MSIs and setup programs, uploads them as artifacts, and runs `test.ps1` against x64. Verify on a pull request that the job passes and the artifacts are attached.

  Done: on PR commit `84b89e2` the `installer` job passed, building both architectures and running `test.ps1` against x64, and attached the `installer` artifact (26 MB).
- [x] 4.3 Make `tools\dev\service.ps1 install` refuse while DeskHop is installed (D8). Verify with the user:
  - with DeskHop installed, the dev install refuses and changes nothing;
  - with the dev service registered, the setup program refuses with the D8 message.

## 5. Docs

- [x] 5.1 Rewrite `installer/README.md`: what the package does, how to build and test it, signing status, and which package suits which PC. Add `installer\build.ps1` and `installer\test.ps1` to AGENTS.md's commands. Update the dev script's help text, which says the installer "takes these steps over later". Verify that the commands as written run.
- [x] 5.2 Add the "Install and uninstall" section to `docs/threat-model.md` (D10), and update its status line. Verify by reading it against `specs/installer` and D3–D6.

## 6. Integration

- [x] 6.1 Run the full checks, and verify that each passes:
  - `cargo fmt --all --check`, `cargo archcheck`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`;
  - the Linux set;
  - the ARM64 check;
  - `npm test`;
  - the ignored desktop tests in `deskhop-service`;
  - the CI run of the `installer` job.

  Done: the local checks passed before the commit, and on PR commit `84b89e2` CI's `linux`, `windows` and `installer` jobs all passed.
- [x] 6.2 Manual check with the user on this PC, with the x64 setup program:
  - **Install:** run the setup program. Expect one consent prompt, progress, then success with **Launch**. **Launch** opens the tray app on *Add a PC*, at medium integrity (Task Manager's *Elevated* column says No).
  - **Running:** the service log shows both agents linked, and Win+L and unlock still switch the secure desktop.
  - **Upgrade:** with the tray app running, run a newer setup program (`build.ps1 -Version 0.1.1`). It asks at most once, with closing preselected, and needs no reboot.
  - **Installed apps:** lists exactly one DeskHop.
  - **Uninstall:** Settings → Apps → DeskHop → Uninstall with the tray app running. It asks at most once, with closing preselected, needs no reboot, and everything in the uninstall requirement is gone.

  Done with the user on 2026-10-07.
  - **First, from the MSI before the setup program existed:**
    - the install, the tray app's first run, the agents and the secure desktop all worked;
    - the dev refusal held in both directions.
    - The prompts that check found led to D11.
  - **Then, with the setup program:**
    - install with **Launch**: the tray app ran unelevated, and the session agent ran as the user while the service and winlogon agent ran as SYSTEM;
    - Installed apps listed one DeskHop;
    - the upgrade to 0.1.1 and the uninstall from Settings, both with the tray app running, each showed the one "Files In Use" dialog, with closing preselected, and needed no reboot;
    - everything was removed.
  - **A bug the check found:** `tools\dev\service.ps1 uninstall` deleted the installed DeskHop's service. It now refuses while DeskHop is installed (D8).
- [ ] 6.3 Manual check on a second PC, and on Windows 10 22H2 if available, deferred with the other two-PC checks:
  - install from the CI artifact;
  - the two PCs pair across a network Windows marked public;
  - an upgrade keeps the pairing.

  If no second PC is available, record that and leave this task open.
