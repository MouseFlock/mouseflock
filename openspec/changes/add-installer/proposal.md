# Proposal

## Why

The design brief's promise starts with "Install on two machines". Today nothing installs DeskHop. A developer runs `tools\dev\service.ps1` from an elevated shell, which builds from source and registers a service named "DeskHop Dev". AGENTS.md says the installer opens the firewall and registers the service, so the user never does either, and `installer/` holds only a README.

The winlogon agent also runs as SYSTEM from the service's folder. Its threat model assumes that folder is under Program Files, where only administrators can write. The installer is what makes that true for real users, and it is the last piece of the MVP.

## What Changes

- **A setup program and a package per architecture,** for x64 and ARM64, built with WiX from the repository by one script:
  - `DeskHop-Setup-<version>-<arch>.exe`, a WiX Burn bundle, is for people. It runs the MSI silently behind its own progress window, owns the Installed apps entry, and offers to start DeskHop when it finishes.
  - `DeskHop-<version>-<arch>.msi` is for silent deployment by administrators.

  Either installs per machine into `C:\Program Files\DeskHop`:
  - `deskhop-service.exe`;
  - `deskhop-agent.exe`;
  - `DeskHop.exe`, the tray app, built in release mode, so it trusts only a service in session 0.
- **The service:**
  - registered as `DeskHop`, running as LocalSystem;
  - starts automatically at boot, so the winlogon agent is at the logon screen;
  - restarts after a failure;
  - started at the end of install, stopped and removed on uninstall.
- **The firewall:** inbound UDP 47391 (QUIC) and 5353 (mDNS), for the service program only, on **all network types**, as agreed with the user.
  - Windows 11 marks a newly joined network Public, home Wi-Fi included, so leaving out Public would break zero configuration.
  - The threat model records what this exposes.
- **Secure attention:**
  - the installer sets the `SoftwareSASGeneration` policy to 1 if it is unset, as the dev script does, so Ctrl+Alt+End from a controlling PC works;
  - it removes the value on uninstall only if it set it, and never changes a value that was already set.
- **The WebView2 runtime the tray app needs** is installed if it is missing.
  - Windows 11 ships it, and most Windows 10 22H2 PCs already have it.
  - If it cannot be installed (no network), the install still succeeds. The service and agents do not need it.
- **Start menu and first run:**
  - a Start menu shortcut for all users;
  - the setup program ends by offering to start the tray app, which shows *Add a PC*. It starts the app as the user who ran it, at normal rights;
  - a silent install opens nothing.
- **Per-user autostart stays as it is**, as agreed with the user. The app registers itself in the user's `Run` key on its first launch, and the tick turns it off. The service needs no user logged on, so other users lose nothing until they first open DeskHop.
- **Upgrade in place:**
  - a newer version replaces an older one without a reboot, closing DeskHop's running programs and restarting the service;
  - it never needs a reboot. Through the setup program it may ask once, with closing DeskHop's programs preselected, as agreed with the user. Run silently it never asks. An MSI run directly with a UI checks for files in use before any elevated step can stop DeskHop, and judges DeskHop's SYSTEM processes critical, so it warns of a reboot. That is why there is a setup program;
  - the data folder (identity, pairings, layouts) is kept;
  - installing an older version over a newer one is refused.
- **Uninstall removes everything**, as agreed with the user: the programs, the service, the firewall rules, the shortcut, and the data folder `C:\ProgramData\DeskHop` with its identity key. Reinstalling makes a new identity that is paired again.
  - The folder is SYSTEM-only, so a step running as SYSTEM must remove it.
  - Any user can create `C:\ProgramData\DeskHop` before DeskHop first starts, for example as a junction pointing elsewhere. So the uninstall does not run a plain recursive delete as SYSTEM.
  - Instead, a small new mode, `deskhop-service --remove-data`, removes the folder only if it is a real folder owned by SYSTEM or Administrators. That is the rule the service already applies at startup. It never follows a link.
- **The installer and the dev registration exclude each other:**
  - the MSI refuses to install while the dev service is registered;
  - `tools\dev\service.ps1 install` refuses while the MSI is installed;
  - each says how to remove the other.
- **CI builds both MSIs and both setup programs, and smoke-tests the x64 ones.** It installs, upgrades and uninstalls the MSI silently, and installs and uninstalls the setup program silently. After each step it checks the service, firewall rules and files.
- **Out of scope:**
  - code signing (SmartScreen will say "Unknown publisher" until a signed release pipeline exists);
  - automatic updates;
  - the `uiAccess` session agent;
  - a machine-wide autostart entry;
  - cleaning other users' `Run` entries and `%APPDATA%` marker on uninstall (a stale `Run` entry for a missing program is ignored by Windows).

## Capabilities

### New Capabilities
- `installer`: what installing, upgrading and uninstalling DeskHop does to a PC. It covers where the programs go, the service registration, the firewall rules, the secure attention policy, the WebView2 runtime, the shortcut and first run, upgrades that keep the identity, uninstall that removes it, exclusion with the dev registration, and how the packages are built and checked.

### Modified Capabilities
- None. The service, agents and tray app behave as their specs already say. The installer only puts them in place, and the desktop app's own autostart rule is unchanged.

## Impact

- **New:**
  - `installer/`: the package project (`DeskHop.wixproj`, `Package.wxs`, `UI.wxs`), the setup program project (`DeskHopSetup.wixproj`, `Bundle.wxs`), `build.ps1` and `test.ps1`;
  - a CI job that builds the x64 and ARM64 MSIs and setup programs, uploads them, and runs the x64 smoke tests.
- **Changed:**
  - `tools/dev/service.ps1`: refuses to install over the MSI;
  - `installer/README.md`, AGENTS.md (the build command), `docs/threat-model.md` (firewall on all profiles, the install folder, uninstall removing the identity);
  - `apps/desktop/src-tauri/tauri.conf.json` only if the release build needs a setting, with `bundle.active` staying `false`. WiX packages it, not Tauri's bundler.
- **One small Rust change:**
  - `win32-session` gains a function that removes a data folder only if it is a real folder with a trusted owner, built on the owner and link checks it already has;
  - `bin/service` gains the `--remove-data` mode that calls it.
  No crate gains a dependency, and `proto`, `model` and `engine` are untouched.
- **Dependency rules hold:**
  - WiX and the .NET SDK are build tools, not crate dependencies;
  - no new `unsafe`;
  - every process the service starts is still started by `launch::agent`;
  - the installer only registers the service and starts it.
- **No grid, no server/client toggle, no IP address field.** The installer asks nothing beyond Windows' own consent prompt.
- **New build requirements:** the .NET SDK (8.0 or later) for the WiX SDK, which `dotnet build` restores from NuGet. Building the ARM64 MSI also needs the MSVC ARM64 build tools to link the release binaries.
