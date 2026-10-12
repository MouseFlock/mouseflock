# Installer

MouseFlock ships, for x64 and for ARM64, as a setup program and a Windows Installer package, both built with WiX 5:

- **`MouseFlock-Setup-<version>-<arch>.exe`** is for people. Run it on each PC; nothing else needs setting up. It runs the package silently behind its own window and owns the *MouseFlock* entry in Installed apps.
- **`MouseFlock-<version>-<arch>.msi`** is for administrators deploying silently (`msiexec /i MouseFlock-….msi /qn`).

Normative requirements: `openspec/specs/installer/spec.md`.

## What it does to a PC

- **Programs:** puts `mouseflock-service.exe`, `mouseflock-agent.exe` and `MouseFlock.exe` (release builds) in `C:\Program Files\MouseFlock`. Only administrators and SYSTEM can change that folder, which matters because the service and its winlogon agent run as SYSTEM from it.
- **Service:**
  - registers the `MouseFlock` service as LocalSystem, starting automatically at boot;
  - restarts it 5 seconds after a crash;
  - starts it when the install finishes.
- **Firewall:** opens inbound UDP 47391 and 5353 for `mouseflock-service.exe` only, on every network type. Windows marks a newly joined network Public, home Wi-Fi included.
- **Secure attention:** sets `SoftwareSASGeneration` to 1 if it is unset, so a controlling PC's Ctrl+Alt+End can show the Ctrl-Alt-Del screen. A value set by someone else is never touched.
- **WebView2:** installs the runtime the tray app needs, if it is missing. If that fails, for example offline, the install still succeeds; the service does not need it.
- **Start menu and first run:**
  - adds a *MouseFlock* Start menu shortcut for all users;
  - the setup program ends with **Launch**, which opens the tray app on *Add a PC* with your normal rights;
  - a silent install starts nothing.

**Upgrade:** running a newer setup program replaces the old version without a reboot, and keeps this PC's identity and pairings. An older version is refused.

**Running programs:**
- **Setup program:** if MouseFlock's tray app or agents are running during an upgrade or uninstall, it shows one "Files In Use" dialog with "Close the applications" preselected. Click OK; no reboot is needed. Run with `/quiet`, it asks nothing.
- **The MSI on its own:** Windows checks for files in use before any of the package's own steps can stop them. Opened directly with its own UI, the MSI may warn that a reboot is needed. One isn't, but use the setup program instead.

**Uninstall** (Settings → Apps → MouseFlock) removes all of the above, including `C:\ProgramData\MouseFlock` with this PC's identity. A reinstalled PC is paired again. The uninstall removes that folder only if it is the real, SYSTEM-owned folder the service made, never through a link.

## Which package

| PC | Setup program | Package for silent deployment |
|---|---|---|
| Windows 10 22H2 or 11 on Intel or AMD | `MouseFlock-Setup-<version>-x64.exe` | `MouseFlock-<version>-x64.msi` |
| Windows 11 on ARM (Snapdragon) | `MouseFlock-Setup-<version>-arm64.exe` | `MouseFlock-<version>-arm64.msi` |

The x64 version also installs on ARM64 Windows and runs under emulation, more slowly. Use the ARM64 one there.

## Building

In any PowerShell, from the repository:

```
installer\build.ps1 -Arch x64      # installer\out\MouseFlock-<version>-x64.msi and MouseFlock-Setup-<version>-x64.exe
installer\build.ps1 -Arch arm64    # the same for arm64
```

**It needs:**
- Rust, with the target installed (`rustup target add aarch64-pc-windows-msvc` for ARM64);
- Node.js;
- the .NET SDK 8 or later, from which `dotnet build` restores WiX from NuGet;
- for ARM64, the MSVC ARM64 build tools.

**What it does:**
- the version comes from the workspace's `Cargo.toml`;
- it downloads Microsoft's WebView2 bootstrapper once into `installer/obj/` and refuses it unless it is validly signed by Microsoft.

## Testing

From an elevated PowerShell, on a PC without MouseFlock installed and without the development service (`tools\dev\service.ps1 uninstall`):

```
installer\build.ps1 -Arch x64 -PackageOnly -OutputSuffix -rebuild
installer\test.ps1 -Msi installer\out\MouseFlock-0.1.0-x64.msi -UpgradeMsi installer\out\MouseFlock-0.1.0-x64-rebuild.msi -Setup installer\out\MouseFlock-Setup-0.1.0-x64.exe
```

**The two packages:** the second is the same build with a new product code, so installing it is an upgrade.

**What the script runs:** it installs, upgrades and uninstalls the MSI, then installs and uninstalls the setup program, all silently.

**What it checks, after each step:** the service, its failure actions, the firewall rules, the files, the shortcut, the Installed apps entry, the data folder and the secure attention policy.

**CI:** runs the same on every change and keeps both packages as the `installer` artifact.

## Development

`tools\dev\service.ps1` registers a development build as the same service name. The two exclude each other: the package refuses to install while the dev service is registered, and the script refuses while the package is installed.

## Not yet

**Unsigned:** the packages are not code-signed, so SmartScreen says "Unknown publisher". To check a package you were given, compare its SHA-256 (`Get-FileHash`) with the one GitHub shows for the CI artifact of the commit it came from.

**Auto-update** comes later.
