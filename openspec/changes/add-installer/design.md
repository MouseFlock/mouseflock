# Design

## Context

- **The dev script** (`tools/dev/service.ps1`) is the only installer today. It shows every step the package must take:
  - `cargo build`, then copy the service and agent to `C:\Program Files\DeskHop Dev`;
  - `New-Service` as LocalSystem;
  - two `New-NetFirewallRule` rules (UDP 47391 and 5353, scoped to the program, every profile, since none is given);
  - `SoftwareSASGeneration` set to 1 when unset;
  - uninstall undoes all of it except the data folder.
- **The service** registers itself with the SCM under the name `DeskHop` (`scm::SERVICE_NAME`).
  - It creates and secures `C:\ProgramData\DeskHop` on start (`store::service_dir`, `win32_session::secure_folder`). That function keeps an existing folder only if it is owned by SYSTEM or Administrators, and moves anything else aside, links included.
  - It finds the agent in its own folder.
  - On stop it ends both agents and waits for them before reporting stopped.
- **The tray app:**
  - built in release mode, `checks::DEV` is false, so it trusts only a service in session 0;
  - `tauri build` adds the `custom-protocol` feature, which a release app needs to load its bundled frontend rather than the dev server, so a plain `cargo build --release` is not enough;
  - `bundle.active` is `false` in `tauri.conf.json`;
  - its autostart is per user (add-onboarding D7), and stays so, as agreed with the user.
- **Tools on hand:** the .NET 8 SDK is installed here and on GitHub's `windows-latest` image. WiX is not installed anywhere, and needs not be: its SDK comes from NuGet through `dotnet build`.
- **Settled with the user:**
  - per-user autostart stays;
  - the firewall opens on every network type;
  - uninstall removes the data folder.

## Goals / Non-Goals

**Goals:**
- Every requirement in `specs/installer`.
- Built from the repository by one script, reproducibly, for x64 and ARM64.
- No step that runs as SYSTEM acts on a path a user can redirect.

**Non-Goals:**
- Code signing, auto-update, per-user installs.
- Changing what the service, agents or app do once installed.

## Decisions

### D1. WiX 5 through the .NET SDK, not Tauri's bundler

`installer/DeskHop.wixproj` uses `WixToolset.Sdk/5.0.2`, with the `Util`, `Firewall` and `UI` extensions at the same version, all pinned. `dotnet build -p:Platform=x64|ARM64` produces `installer/out/DeskHop-<version>-<arch>.msi`.

- **Alternative: Tauri's built-in WiX bundler.** It targets WiX 3 and builds an app-centric package. Registering a service, firewall rules, a SYSTEM custom action and per-architecture service binaries would all be custom fragments in Tauri's template, and Tauri's packaging defaults would still have to be argued with. A standalone project is clearer and is what AGENTS.md describes. Rejected.
- **Alternative: WiX 3 (candle/light).** It is legacy, and its tools would have to be installed on every machine. Rejected.
- **Pinning.** WiX 5.0.2 is pinned for reproducible builds. Moving to a later major is its own change, after checking its license terms.

### D2. Building: `installer/build.ps1 -Arch x64|arm64`

1. **Version:** read from `cargo metadata` (the workspace package version, `0.1.0` today) and pass it to WiX as `ProductVersion`.
2. **Tray app:** in `apps/desktop`, `npm ci` if `node_modules` is missing, then `npx tauri build --no-bundle --target <triple>`. This runs the frontend build and gives `target/<triple>/release/DeskHop.exe`.
3. **Service and agent:** `cargo build --release --locked -p deskhop-service -p deskhop-agent --target <triple>`.
4. **WebView2 bootstrapper (D6):**
   - download it once into `installer/obj/`, which is ignored by git;
   - check that its Authenticode signature is valid and that the signer is Microsoft;
   - otherwise stop.
5. **Package:** `dotnet build installer/DeskHop.wixproj -c Release -p:Platform=<x64|ARM64> -p:ProductVersion=… -p:BinDir=target/<triple>/release`.

The triples are `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`. The script prints each step and stops on the first failure.

### D3. The package (`Package.wxs` and fragments)

- **`<Package>`:**
  - `Scope="perMachine"`, `InstallerVersion="500"`, `Compressed="yes"`, with one embedded cabinet;
  - one fixed `UpgradeCode` for both architectures;
  - `Manufacturer="DeskHop"`.
- **`<MajorUpgrade>`:**
  - `AllowSameVersionUpgrades="yes"`, so CI and dev rebuilds of one version can be reinstalled;
  - `DowngradeErrorMessage` "A newer version of DeskHop is already installed.";
  - scheduled `afterInstallInitialize`, so the old product is removed inside the same transaction (rollback restores it on failure).
- **Files:**
  - `INSTALLFOLDER` = `[ProgramFiles64Folder]DeskHop`. On ARM64, `ProgramFiles64Folder` is `C:\Program Files` too.
  - One component per file, so each keeps a stable key path.
  - The folder inherits Program Files' ACL, so only administrators and SYSTEM can write. The package sets no permissions of its own.
- **Service:**
  - `<ServiceInstall Name="DeskHop" Account="LocalSystem" Start="auto" Type="ownProcess" ErrorControl="normal">` on `deskhop-service.exe`, with a description;
  - `<ServiceConfig>` failure actions: restart after 5 s for the first, second and later failures, with the counter reset after a day;
  - `<ServiceControl Start="install" Stop="both" Remove="uninstall" Wait="yes">`.
  - Not "delayed auto start": the winlogon agent should be at the logon screen as soon as possible after boot.
- **Firewall** (`fw:FirewallException`):
  - "DeskHop (UDP 47391)" and "DeskHop (mDNS)";
  - `Program="[#ServiceExe]"`, `Protocol="udp"`, `Port` 47391 or 5353, `Scope="any"`, `Profile="all"`.
  - The names differ from the dev script's "DeskHop Dev (…)" rules, so the two never collide.
- **Start menu:** a `DeskHop` shortcut in `ProgramMenuFolder` to `DeskHop.exe`. Its HKLM registry key path, `HKLM\SOFTWARE\DeskHop\StartMenuShortcut`, makes it a per-machine component.
- **Running programs:**
  - `util:CloseApplication` for `DeskHop.exe`, with `TerminateProcess="1"` and a short timeout, scheduled for install and uninstall. The app hides on `WM_CLOSE` rather than exiting, so termination is the path that works.
  - Its state lives in the service, so nothing is lost. The tray icon may linger until hovered.
  - The agents need nothing: stopping the service ends them before `StopServices` returns (Context).
  - **Files in use: why people get the setup program (D11).**
    - Windows Installer looks for files in use at `InstallValidate`. That is before `InstallInitialize`, and so before any step that runs elevated. It always finds the agents and any tray app holding their programs.
    - **Run with a UI, the check fails.** The 6.2 check, with verbose logs, showed this. An upgrade's removal of the old version made the check with a UI involved, decided "a critical application holds file[s] in use, so a reboot will be necessary", and showed the reboot warning. The non-elevated side cannot see into DeskHop's SYSTEM processes.
      - Settings' Uninstall for an MSI uses the basic UI, with the same result.
      - Every variation failed:
        - turning Restart Manager off fell back to `FilesInUse`, which listed the winlogon agent;
        - removing that dialog brought up Windows' own "should be closed" box.
    - **Run silently, the check succeeds.** In the same state, with the tray app running, the log reads "Successfully shut down all applications that held files in use". Restart Manager also leaves the service to `ServiceControl` ("will be stopped due to a service control action authored in the package"), and the run returns 0 with nothing pending a reboot.
    - So people use the setup program (D11). It runs the MSI with no UI of its own, and asks at most once, with closing preselected and never a reboot. The MSI keeps `MSIRMSHUTDOWN=1`, so a program that does not answer is ended.
    - Run on its own with a UI, the MSI may still show Windows' files-in-use message. The spec allows that, and the README says to use the setup program.
  - `CloseApplication` (before `StopServices`) and `StopServices` still make sure nothing holds a file when it is replaced or removed, since the service relaunches an agent that Restart Manager ends.
- **Markers:** `HKLM\SOFTWARE\DeskHop`:
  - `InstallFolder`, which the dev script checks (D8);
  - `SasSet` (D5).

### D4. Removing the data folder: `deskhop-service --remove-data`

The data folder is SYSTEM-only, so only a deferred custom action that is not impersonated (it runs as SYSTEM) can remove it. Any user can create folders in `C:\ProgramData`. So until the service first starts, `C:\ProgramData\DeskHop` could be a user's junction to, say, `C:\Windows`. A plain `rmdir /s` as SYSTEM, or WiX's `RemoveFolderEx`, would then act on the target. `RemoveFolderEx` also enumerates in an immediate action that cannot read a SYSTEM-only folder.

- **`win32_session::remove_secure_folder(path)`**, next to `secure_folder`, reusing its owner and link checks:
  - `Ok(Removed)`: `path` was a real folder, not a reparse point, owned by SYSTEM or Administrators. Its contents were removed with `std::fs::remove_dir_all`, which does not follow links or junctions inside the tree.
  - `Ok(Absent)`: nothing was there.
  - `Ok(Refused)`: a link or junction, or a folder with another owner. It is left untouched, and so is anything it points to.
  - `Err`: an I/O failure.
- **`deskhop-service --remove-data`:**
  - a new `Mode` in `main.rs`;
  - removes `store::service_dir()` and prints what it did;
  - always exits 0, so a leftover never blocks an uninstall. The MSI log records the output.
- **The custom action:**
  - WiX Util's quiet-exec custom action (`WixQuietExec`, from the util extension's DLL for the package's platform), deferred, `Impersonate="no"`, `Return="ignore"`;
  - runs `"[#ServiceExe]" --remove-data`;
  - scheduled after `StopServices` and before `RemoveFiles`, while the program still exists;
  - condition `REMOVE~="ALL" AND NOT UPGRADINGPRODUCTCODE`, so an upgrade, which uninstalls the old version, keeps the identity.
- **Alternative: a PowerShell or cmd script as SYSTEM.** Reproducing the owner and reparse checks there is fragile, and they already exist, tested, in `win32-session`. Rejected.

### D5. The secure attention policy without clobbering

- **Searches:**
  - `RegistrySearch` `SAS_EXISTING` for `HKLM\…\Policies\System\SoftwareSASGeneration`;
  - `SAS_OURS` for `HKLM\SOFTWARE\DeskHop\SasSet`.
- **One component** writes `SoftwareSASGeneration=1` and `SasSet=1`, with condition `NOT SAS_EXISTING OR SAS_OURS`. Windows Installer removes component-owned values on uninstall.
- **What each case gets:**
  - *Unset before install:* the component installs, and uninstall removes both values.
  - *Set by someone else:* the condition is false, and the values are never touched.
  - *Upgrade:* `SAS_OURS` is found, so the new version installs the component again and keeps the value. The old version's removal and the new version's install run in one transaction.

### D6. WebView2: the evergreen bootstrapper, only when missing

- **Embedding:** `MicrosoftEdgeWebview2Setup.exe` (about 2 MB), Microsoft's redistributable online bootstrapper, is embedded as a `<Binary>`. It is architecture-neutral and installs the runtime that matches the machine.
- **Searches:** `WEBVIEW2` looks for the `pv` value under `HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}`, then under the same key without `WOW6432Node`. Microsoft documents both as places the runtime registers.
- **Running it:** a deferred custom action, not impersonated, runs it with `/silent /install`, condition `NOT WEBVIEW2 AND NOT REMOVE`, `Return="ignore"`. A failure, such as being offline, leaves the install successful, and the MSI log records the exit code.
- **Alternative: the setup program (D11) installs the runtime instead.** The MSI would no longer stand alone for administrators deploying it. Rejected: it stays in the MSI.
- **Alternative: the 120 MB offline runtime.** Rejected for size, since Windows 11 already has the runtime.

### D7. The MSI's own UI, for when it is run directly

People use the setup program's UI (D11). The MSI keeps a minimal UI of its own for anyone who opens it directly:

- **Dialogs:**
  - a small custom `<UI>` built from WixUI's stock dialogs: `ProgressDlg`, `ExitDialog`, `FatalError`, `UserExit`, `FilesInUse` and `MsiRMFilesInUse`. The last two are Windows' files-in-use dialogs (D3);
  - no welcome, licence or folder pages, so the install asks nothing;
  - the exit dialog shows the optional checkbox "Start DeskHop", checked by default (`WIXUI_EXITDIALOGOPTIONALCHECKBOX`).
- **Starting the app:** the checkbox's `DoAction` runs `WixShellExec` on `[#DeskHopExe]`.
  - It runs in the UI sequence, in msiexec's client process. For a package opened by double-clicking, that is the user's own, unelevated process; only the server elevates.
  - The app therefore starts at normal integrity in the user's session. A package started from an elevated prompt passes that elevation on, which the spec allows.
- **Silent installs:** `/qn` shows no UI, so nothing starts.

### D8. Excluding the dev registration

- **The package:**
  - `RegistrySearch` `DESKHOP_SERVICE_IMAGE` for `HKLM\SYSTEM\CurrentControlSet\Services\DeskHop\ImagePath`;
  - `<Launch Condition="Installed OR WIX_UPGRADE_DETECTED OR NOT DESKHOP_SERVICE_IMAGE">`;
  - message: "DeskHop's development service is registered. Run `tools\dev\service.ps1 uninstall` from an elevated PowerShell first."
- **`tools\dev\service.ps1 install` and `uninstall`:** both refuse when `HKLM\SOFTWARE\DeskHop\InstallFolder` exists, and change nothing.
  - `uninstall` too: it removes the service by its name, which is the package's own. The 6.2 check found it deleting the installed DeskHop's service.

### D9. Checking: `installer/test.ps1` and CI

**`installer/test.ps1 -Msi <path>`** needs an elevated shell.
1. **Install:** `msiexec /i <msi> /qn /l*v <log>`. Then check:
   - exit 0;
   - the service is `Running`, `Automatic`, as LocalSystem, and points into `C:\Program Files\DeskHop`;
   - both firewall rules exist, scoped to that program, UDP, profile `Any`;
   - the three files and the Start menu shortcut exist;
   - `SoftwareSASGeneration` is 1 if it was unset before;
   - `C:\ProgramData\DeskHop` exists within 10 s, created by the service.
2. **Upgrade:** reinstall the same MSI, an upgrade thanks to `AllowSameVersionUpgrades`. Then check:
   - the service is running again;
   - the data folder's creation time is unchanged, so the identity was kept.
3. **Uninstall:** `msiexec /x <msi> /qn`. Then check:
   - the service, rules, files, shortcut, `HKLM\SOFTWARE\DeskHop` and the data folder are gone;
   - the SAS value is back to its state before the install.

4. **Setup program,** with `-Setup <exe>`:
   - `<exe> /quiet /norestart /log <log>`, then the install checks, plus exactly one *DeskHop* entry in Installed apps;
   - `<exe> /uninstall /quiet /norestart /log <log>`, then the uninstall checks.

It prints each check and exits non-zero on the first failure.

**CI** gets a new `installer` job on `windows-latest`:
- Node, the Rust toolchain with the `aarch64-pc-windows-msvc` target, and the preinstalled .NET SDK;
- `build.ps1` for both architectures, with both MSIs and both setup programs uploaded as artifacts;
- `test.ps1` against the x64 MSI and setup program. Runners are elevated, and the service runs there with no user logged on.

**Unit tests:** `remove_secure_folder` gets tests for:
- an absent path;
- a junction to a temp folder whose file must survive (made with `cmd /c mklink /J`, which needs no privilege);
- a folder owned by this user, refused because the owner is untrusted.

The removal path itself needs SYSTEM ownership, so the CI smoke test covers it.

### D10. Threat model

`docs/threat-model.md` gains an "Install and uninstall" section:
- **Install folder:** Program Files, admin-only. The winlogon agent's SYSTEM launch relies on this.
- **Firewall:** rules on every profile, scoped to the service program. On a public network, any host there reaches `proto` decoding and the QUIC handshake. That is what the earlier sections already defend, with pairing open only on the user's request.
- **WebView2 bootstrapper:** runs as SYSTEM during install. It is a Microsoft-signed binary whose signature the build checks, and it downloads from Microsoft.
- **Uninstall:** removes the identity key. The SYSTEM removal step refuses links and untrusted owners (D4).
- **Policy:** the SAS policy is set only when unset.

### D11. The setup program: a Burn bundle around the MSI

Added after the 6.2 check (D3), as agreed with the user: an MSI run with a UI cannot handle DeskHop's running programs without a reboot warning, and run silently it handles them cleanly.

- **`installer/DeskHopSetup.wixproj`** (`OutputType` `Bundle`, the same pinned `WixToolset.Sdk/5.0.2`, plus `WixToolset.Bal.wixext` and `WixToolset.Util.wixext`) and **`Bundle.wxs`**.
  - `build.ps1` builds it right after the MSI, for the same platform, into `installer/out/DeskHop-Setup-<version>-<arch>.exe`.
- **`<Bundle>`:**
  - name "DeskHop", manufacturer "DeskHop", the workspace version, its own fixed `UpgradeCode`, and the app icon;
  - it owns the Installed apps entry.
- **Chain:** one `<MsiPackage>`, the MSI, `Visible="no"`. Burn runs it with no UI of its own and handles its messages, and Burn marks the MSI a system component, so Installed apps lists only the bundle.
  - **Files in use.** The MSI's files-in-use message reaches the bootstrapper UI. The 6.2 check showed WiX 5's standard UI turning it into one "Files In Use" dialog, listing the agents and the tray app, with "Close the applications" preselected. After OK, no reboot is needed, and WiX 5 has no setting to skip the dialog. The user kept this one prompt.
  - Run with `/quiet`, nothing asks.
  - On uninstall, Burn removes the MSI the same silent way.
  - The bundle's per-machine chain is elevated once, through Windows' consent prompt.
- **UI:** `bal:WixStandardBootstrapperApplication` with the `hyperlinkLicense` theme and no licence link, so there is nothing to accept, and no options page.
  - It shows Install, then progress, then success with **Launch**.
  - `LaunchTarget="[ProgramFiles64Folder]DeskHop\DeskHop.exe"`: the bootstrapper application runs unelevated as the user (Burn's engine elevates separately), so the tray app starts with the user's rights.
- **Upgrades:** a newer bundle with the same `UpgradeCode` replaces the older one. Its MSI's major upgrade replaces the old MSI (D3), and Burn then removes the old bundle's registration. Downgrades are refused by Burn's related-bundle check and by the MSI.
- **Dev registration (D8):**
  - `util:RegistrySearch` reads the `DeskHop` service's `ImagePath` into a variable;
  - `bal:Condition` refuses when it contains `DeskHop Dev`, the dev script's folder, with the same message as the MSI's launch condition, shown in the bundle's UI;
  - the MSI's own launch condition stays for direct installs.
- **The MSI stays complete on its own** for silent deployment (`msiexec /qn`), with its own Installed apps entry when installed directly. Its UI (D7) remains for anyone who opens it directly, with the files-in-use caveat the README states.
- **Alternative: a Rust setup program** with an embedded MSI. It would need its own elevation and process handling, and `unsafe` outside the four allowed crates. Rejected: Burn is WiX's own tool for this.
- **Alternative: a "stop DeskHop" step in the chain before the MSI,** which would remove even the one prompt. It needs a new `deskhop-service` mode, and it must avoid stopping the new version during the old bundle's removal. Rejected by the user in favour of the one prompt.

## Risks / Trade-offs

- **[Unsigned packages]** SmartScreen shows "Unknown publisher", and some organisations block unsigned MSIs. → Signing comes with a release pipeline (a later change). Until then, the README says how to check the package's hash against the CI artifact.
- **[ARM64 linking]** It needs the MSVC ARM64 build tools. GitHub's Windows image has them; a developer's machine may not. → `build.ps1` checks for the ARM64 libraries and says how to install them. CI is the reference build for ARM64, and no ARM64 hardware is available to run that package, so its install is checked only in the deferred multi-PC pass.
- **[x64 package on ARM64 Windows]** Windows Installer has no clear property for "running on ARM64", so the x64 package may install there and run under emulation. → Allowed: it works, more slowly. The README points ARM64 users at the ARM64 package.
- **[Terminating the tray app on upgrade]** A ghost tray icon may remain until hovered. The app is not restarted after a silent upgrade until the next logon or a Start menu launch. → After an interactive upgrade, the setup program's **Launch** starts it again.
- **[Same-version bundles]** Burn may treat a rebuilt bundle of the same version as the installed one rather than an upgrade. → The MSI's own upgrade path is tested with two same-version MSIs. Real releases always raise the version.
- **[Stale per-user entries after uninstall]** Other users' `Run` values and `%APPDATA%\com.deskhop.desktop` stay behind. Windows skips a `Run` entry whose program is missing. → Allowed by the spec.
- **[WebView2 download fails]** The tray app cannot start until the runtime is present, while the service and pairing still work and pairing can be done from another PC's app. → The MSI log records it, and rerunning the package (a repair) tries again.
- **[First install on a CI runner differs from a desktop]** There is no console user, so no session agent is checked. → The manual check covers a real desktop.

## Migration Plan

- **Developers using the dev script:** run `tools\dev\service.ps1 uninstall` once, then install with the setup program. D8 enforces the order in both directions.
- **Release:** none exists yet, so nothing else migrates.

## Open Questions

- Whether `WixToolset.Firewall`'s `Profile="all"` also covers rules Windows creates per profile on Windows 10. The smoke test checks the resulting rule's profile on the CI image (Windows Server 2022/2025). A Windows 10 machine is checked in the deferred multi-PC pass. If it differs, only the fragment changes, not the spec.
