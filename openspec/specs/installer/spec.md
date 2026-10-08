# installer Specification

## Purpose

What installing, upgrading and uninstalling DeskHop does to a PC. One install puts the service, the agent and the tray app in place, registers the service and opens the firewall, so the user never does either. Uninstalling takes all of it away again.

## Requirements

### Requirement: A setup program and a package per architecture
For each of x64 and ARM64, DeskHop SHALL ship two files. `DeskHop-Setup-<version>-<arch>.exe` is the setup program, for people. `DeskHop-<version>-<arch>.msi` is a Windows Installer package, for silent deployment. The setup program SHALL install by running that package silently behind its own progress window. Either SHALL install per machine into `C:\Program Files\DeskHop`, asking nothing beyond Windows' administrator consent.

#### Scenario: Install on x64
- **WHEN** a user runs the x64 setup program on Windows 11 x64
- **THEN** `deskhop-service.exe`, `deskhop-agent.exe` and `DeskHop.exe`, release builds for x64, are in `C:\Program Files\DeskHop`, and only administrators and SYSTEM can change that folder

#### Scenario: Release tray app
- **WHEN** the installed `DeskHop.exe` finds a UI pipe served from a user's session rather than session 0
- **THEN** it does not connect, as a release build of the app requires

### Requirement: The service is registered and running
The install SHALL register the service `DeskHop` to run `deskhop-service.exe` from the install folder as LocalSystem, starting automatically at boot. It SHALL start the service at the end of the install. On uninstall it SHALL stop the service and remove it.

#### Scenario: Running after install
- **WHEN** the install finishes
- **THEN** the `DeskHop` service is running as LocalSystem with automatic start, and within 2 seconds a winlogon agent runs in the console session

#### Scenario: Logon screen after a reboot
- **WHEN** the PC restarts after the install and no one has logged on
- **THEN** the service is running and its winlogon agent is at the logon screen

### Requirement: The service restarts after a failure
The service SHALL be registered to restart 5 seconds after it ends unexpectedly, for the first three failures in a day.

#### Scenario: Service process killed
- **WHEN** an administrator kills `deskhop-service.exe`
- **THEN** the service is running again within 10 seconds

### Requirement: Firewall rules on every network type
The install SHALL add inbound firewall rules that allow UDP port 47391 and UDP port 5353 to `deskhop-service.exe` in the install folder. The rules SHALL apply to domain, private and public networks, and SHALL allow no other program or port. Uninstall SHALL remove them.

#### Scenario: Home network marked public
- **WHEN** DeskHop is installed on two PCs whose Wi-Fi Windows classified as a public network
- **THEN** they find each other and can pair without any firewall change by the user

#### Scenario: Only the service
- **WHEN** another program on the PC listens on UDP 47391
- **THEN** the DeskHop rules do not let traffic reach it

### Requirement: Secure attention policy
When the `SoftwareSASGeneration` policy is not set, the install SHALL set it to 1, so services may show the Ctrl-Alt-Del screen. When it is already set, the install SHALL leave it unchanged. Uninstall SHALL remove the value only if the install set it, and an upgrade SHALL keep it.

#### Scenario: Policy already set
- **WHEN** an administrator set `SoftwareSASGeneration` to 0 before installing, then installs and uninstalls DeskHop
- **THEN** the value is 0 throughout

#### Scenario: Policy unset
- **WHEN** the value is not set, and DeskHop is installed and later uninstalled
- **THEN** it is 1 while DeskHop is installed, and not set after the uninstall

### Requirement: The WebView2 runtime is present
When the Microsoft Edge WebView2 runtime is not installed, the install SHALL install it. If that fails, for example without a network, the install SHALL still succeed with the service running, and the log SHALL record the failure.

#### Scenario: Runtime missing and offline
- **WHEN** DeskHop is installed on a PC without the WebView2 runtime and without internet access
- **THEN** the install succeeds, and the service and agents run

### Requirement: Start menu and first run
The install SHALL add a *DeskHop* shortcut to the Start menu for all users. When the setup program finishes an install or upgrade, it SHALL offer to start `DeskHop.exe`. If the user accepts, it SHALL start it as the user who ran the setup program, with that user's rights rather than elevated ones. A silent install SHALL start nothing in any user's session.

#### Scenario: Interactive install
- **WHEN** a user runs the setup program on a PC that has never paired and accepts the offer to start DeskHop
- **THEN** the tray app opens on *Add a PC*, running at the user's normal integrity

#### Scenario: Silent install
- **WHEN** an administrator installs DeskHop with `msiexec /qn` or the setup program's `/quiet`
- **THEN** the service runs and no tray app starts

### Requirement: Running programs never need a reboot
An upgrade or uninstall SHALL close DeskHop's running programs (the tray apps and the agents) and stop the service before replacing or removing files, and SHALL never need a reboot. Through the setup program it MAY ask once, with closing the programs preselected. Run silently it SHALL NOT ask. Only the package run on its own with a user interface MAY show Windows' own message about files in use.

#### Scenario: Upgrade with the tray app running
- **WHEN** the tray app is running and the user runs a newer setup program
- **THEN** it asks at most once, with closing preselected, the upgrade finishes without a reboot, and the setup program offers to start DeskHop again

#### Scenario: Uninstall from Settings with the tray app running
- **WHEN** the tray app is running and the user uninstalls DeskHop from Settings
- **THEN** it asks at most once, with closing preselected, and DeskHop is removed without a reboot

#### Scenario: Silent upgrade
- **WHEN** an administrator upgrades with `msiexec /qn` or the setup program's `/quiet` while DeskHop's programs run
- **THEN** nothing asks, and the upgrade finishes without a reboot

### Requirement: Upgrade in place
Installing a newer version SHALL replace the installed one, stopping the service, replacing the programs and starting the service again. The data folder `C:\ProgramData\DeskHop` SHALL be kept, so the PC keeps its identity, members and layouts.

#### Scenario: Upgrade keeps pairing
- **WHEN** a PC paired with another installs a newer DeskHop version
- **THEN** after the upgrade both PCs report each other up again without pairing

### Requirement: No downgrade
Installing a version older than the one installed SHALL be refused with a message that a newer version is installed, changing nothing.

#### Scenario: Older package
- **WHEN** version 0.2.0 is installed and the user runs the 0.1.0 setup program
- **THEN** it says a newer version is installed, and 0.2.0 keeps running

### Requirement: Uninstall removes everything
Installed apps SHALL list one *DeskHop* entry. Its uninstall SHALL remove the programs, the install folder, the service, the firewall rules and the Start menu shortcut. It SHALL also remove the data folder `C:\ProgramData\DeskHop`, with the identity key, members, layouts and log.

It SHALL remove the data folder only if it is a real folder owned by SYSTEM or Administrators, and SHALL never follow a link while removing it. Per-user files and `Run` entries outside the install MAY remain.

#### Scenario: Uninstall then reinstall
- **WHEN** a paired PC uninstalls DeskHop and installs it again
- **THEN** it has a new identity, no members, and must be paired again

#### Scenario: Data folder replaced by a junction
- **WHEN** a user replaced `C:\ProgramData\DeskHop` with a junction to another folder before the uninstall
- **THEN** the uninstall finishes, and nothing in the junction's target is deleted

### Requirement: The installer and the development registration exclude each other
The package SHALL refuse to install while a service named `DeskHop` that it did not install is registered, and SHALL say to run `tools\dev\service.ps1 uninstall` first. The development script SHALL refuse to register or remove its service while DeskHop is installed, since the service it would remove is DeskHop's, and SHALL say to use Settings instead.

#### Scenario: Dev service present
- **WHEN** a developer registered the service with `tools\dev\service.ps1 install` and then runs the setup program
- **THEN** the install stops before changing anything and says how to remove the dev registration

### Requirement: Packages are built and checked from the repository
One script in `installer/` SHALL build the package and the setup program for a given architecture from the repository, with the version taken from the workspace version. On every change, CI SHALL build them for both architectures. It SHALL then install, upgrade and uninstall the x64 package silently on a CI runner, and install and uninstall the x64 setup program silently. It SHALL check the service, the firewall rules and the files after each step.

#### Scenario: CI smoke test
- **WHEN** CI installs DeskHop silently and then uninstalls it
- **THEN** after install the service is running and the rules and files exist, and after uninstall none of them, nor the data folder, remain
