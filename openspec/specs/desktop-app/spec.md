# desktop-app Specification

## Purpose

`DeskHop.exe` is the optional app through which a user pairs PCs, sees their machines and takes back a learned side. It is a client of the service's UI link. It holds no engine logic, and closing it changes nothing.

## Requirements

### Requirement: Trust only the service
Before sending `Hello` on `\\.\pipe\deskhop-ui`, the app SHALL check the session of the pipe's server. A release build SHALL proceed only if the server runs in session 0. A development build SHALL also accept a server in the app's own session, for console mode. Any other server SHALL be disconnected unread. The app SHALL then report that it is not connected, and retry as when the service is unreachable.

#### Scenario: Service in session 0
- **WHEN** the registered service holds the UI pipe and a release build of the app starts
- **THEN** the app connects and shows the service's status

#### Scenario: Pipe held by a user process
- **WHEN** a process in the user's session holds the UI pipe and a release build of the app starts
- **THEN** the app sends nothing on it, shows no status from it, and reports that it is not connected to the service

#### Scenario: Console mode during development
- **WHEN** `deskhop-service --console` holds the UI pipe and a development build of the app starts in the same session
- **THEN** the app connects and shows the console-mode service's status

### Requirement: Service not reachable
When the app cannot reach the service, or its link to the service closes, it SHALL say in the window and in the tray tooltip that DeskHop's service isn't running. It SHALL retry about once a second, and show the service's status again as soon as it connects. While not connected it SHALL offer no command that needs the service.

#### Scenario: Service started after the app
- **WHEN** the app is running and the service is stopped, and then the service starts
- **THEN** within 2 seconds the app shows the service's status

#### Scenario: Service stops
- **WHEN** the service stops while the app is connected
- **THEN** the app says the service isn't running, and *Add a PC*, *Remove* and *Forget layout* are unavailable

### Requirement: Show the latest status
While connected, the window SHALL show what the latest `Status` from the service holds: this PC's name, and every member with its name and whether it is up. Members with the same name SHALL be told apart by a short form of their key. The app SHALL NOT show any machine's network address, and SHALL keep no record of its own of machines or layout.

#### Scenario: Machine goes down
- **WHEN** the window is open and the service reports member B as down
- **THEN** the window shows B as not connected

#### Scenario: Two machines with one name
- **WHEN** the desk has two members both named `OFFICE`
- **THEN** each is shown with a different short key beside its name

### Requirement: Tray icon
While the app runs it SHALL show a tray icon. The tooltip SHALL say whether the service is reachable, how many members are up, and which member, if any, has this PC's input or controls this PC. Clicking the icon SHALL open the window. Its menu SHALL offer *Open DeskHop*, *Add a PC*, *Start with Windows* (a tick) and *Quit*.

#### Scenario: Controlling a peer
- **WHEN** this PC's input is on member B
- **THEN** the tooltip names B as the machine being controlled

#### Scenario: Add a PC from the tray
- **WHEN** the user picks *Add a PC* in the tray menu
- **THEN** the window opens on *Add a PC*

### Requirement: Closing changes nothing
Closing the window SHALL hide it, and the app SHALL keep running in the tray. *Quit* SHALL end the app only. Neither SHALL send any command to the service, or change pairing, members, layout or focus.

#### Scenario: Quit while controlling a peer
- **WHEN** this PC's input is on member B and the user quits the app
- **THEN** input stays on B and the service carries on unchanged

### Requirement: One app per user
At most one instance of the app SHALL run per user session. Starting the app again SHALL show the running instance's window and start nothing new.

#### Scenario: Started twice
- **WHEN** the app is running in the tray and the user starts `DeskHop.exe` again
- **THEN** the existing window opens and only one tray icon is shown

### Requirement: First run opens Add a PC
When the app starts, connects, and the first `Status` lists no members, the window SHALL open on *Add a PC*. When that `Status` lists members, the app SHALL start in the tray without opening the window.

#### Scenario: Fresh install
- **WHEN** the app starts on a PC that has never paired
- **THEN** the window opens on *Add a PC*

#### Scenario: Paired PC at logon
- **WHEN** the app starts at logon on a PC with members
- **THEN** only the tray icon appears

### Requirement: Show a code
*Show a code* SHALL send `OpenPairing`. While the service reports pairing open, the window SHALL show the code and the time left, counting down. *Cancel* SHALL send `ClosePairing`. When pairing closes, the window SHALL say why. When pairing cannot open, it SHALL say the PC needs a network connection.

#### Scenario: Code shown
- **WHEN** the user picks *Show a code* and the service opens pairing with code `137-4829153`
- **THEN** the window shows `137-4829153` and about 10 minutes left

#### Scenario: Other PC joins
- **WHEN** a code is shown and a machine joins with it
- **THEN** the window says the new machine's name and lists it as a member

#### Scenario: No network
- **WHEN** the user picks *Show a code* and the service answers `PairingNotOpened`
- **THEN** the window says to connect this PC to a network first

#### Scenario: Too many wrong codes
- **WHEN** the service reports pairing closed after too many wrong codes
- **THEN** the window says so and offers a new code

### Requirement: Enter a code
*Enter a code* SHALL take the code the user types, remove whitespace from it, and send it in `Join`. Text that the UI link cannot carry SHALL be reported as not a valid code without being sent. The window SHALL show that joining is in progress until `Paired` or `JoinFailed`. It SHALL then name the machine joined, or give the failure reason in plain words and keep the typed code for correction.

#### Scenario: Joined
- **WHEN** the user enters the code shown on PC A and the service reports `Paired` naming A
- **THEN** the window says this PC joined A and lists A as a member

#### Scenario: Typo
- **WHEN** the user enters a code with a wrong check digit
- **THEN** the window says the code has a typo, keeps the typed code, and no attempt is counted on the other PC

#### Scenario: Spaces in the code
- **WHEN** the user types `137 - 482 9153`
- **THEN** the app sends `137-4829153`

#### Scenario: Too long to be a code
- **WHEN** the user types 40 characters
- **THEN** the window says it is not a valid code and nothing is sent

#### Scenario: Other PC not showing a code
- **WHEN** the service reports `JoinFailed` with reason not ready
- **THEN** the window says to pick *Show a code* on the other PC first

### Requirement: Remove a machine
Each member SHALL offer *Remove*. After the user confirms, the app SHALL send `RemovePeer` with that member's key. Without confirmation it SHALL send nothing.

#### Scenario: Remove confirmed
- **WHEN** the user picks *Remove* on member B and confirms
- **THEN** the app sends `RemovePeer` with B's key, and the window drops B once the service's status no longer lists it

#### Scenario: Remove cancelled
- **WHEN** the user picks *Remove* on member B and cancels
- **THEN** nothing is sent

### Requirement: Forget layout
The window SHALL offer *Forget layout*. After the user confirms, the app SHALL send `ForgetLayout`, and say that the next push across an edge will learn it again.

#### Scenario: Forget confirmed
- **WHEN** the user picks *Forget layout* and confirms
- **THEN** the app sends `ForgetLayout`

### Requirement: Undo popup
When the service reports `Learned`, the app SHALL show a popup near the tray. It SHALL name the member and the side, offer *Undo*, and count down the time left, starting from the time the notice carries. *Undo* SHALL send `Undo` and close the popup. The popup SHALL close when the time runs out. A newer `Learned` SHALL replace it. The popup SHALL NOT take keyboard focus.

#### Scenario: Learned and undone
- **WHEN** the service reports that B was learned to the right with 10,000 ms left, and the user picks *Undo* 4 seconds later
- **THEN** the popup said B is to the right, the app sends `Undo`, and the popup closes

#### Scenario: Learned and kept
- **WHEN** the service reports a learned side and the user does nothing
- **THEN** the popup closes when its countdown reaches zero and nothing is sent

#### Scenario: Typing elsewhere
- **WHEN** the popup appears while the user is typing in another window on this PC
- **THEN** the keystrokes keep going to that window

### Requirement: Start with Windows
On its first launch for a user, the app SHALL register itself to start when that user logs on. The tray's *Start with Windows* tick SHALL show whether it is registered, and toggling it SHALL add or remove the registration. After the user turns it off, the app SHALL NOT turn it on again by itself. A development build SHALL NOT register itself without the tick.

#### Scenario: First launch
- **WHEN** a user launches the app for the first time
- **THEN** the app starts at that user's next logon

#### Scenario: Turned off
- **WHEN** the user unticks *Start with Windows*, then quits and launches the app again
- **THEN** it is still unticked and the app does not start at logon
