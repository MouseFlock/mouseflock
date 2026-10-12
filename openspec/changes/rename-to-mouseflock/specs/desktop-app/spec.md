## MODIFIED Requirements

### Requirement: Trust only the service
Before sending `Hello` on `\\.\pipe\mouseflock-ui`, the app SHALL check the session of the pipe's server. A release build SHALL proceed only if the server runs in session 0. A development build SHALL also accept a server in the app's own session, for console mode. Any other server SHALL be disconnected unread. The app SHALL then report that it is not connected, and retry as when the service is unreachable.

#### Scenario: Service in session 0
- **WHEN** the registered service holds the UI pipe and a release build of the app starts
- **THEN** the app connects and shows the service's status

#### Scenario: Pipe held by a user process
- **WHEN** a process in the user's session holds the UI pipe and a release build of the app starts
- **THEN** the app sends nothing on it, shows no status from it, and reports that it is not connected to the service

#### Scenario: Console mode during development
- **WHEN** `mouseflock-service --console` holds the UI pipe and a development build of the app starts in the same session
- **THEN** the app connects and shows the console-mode service's status

### Requirement: Service not reachable
When the app cannot reach the service, or its link to the service closes, it SHALL say in the window and in the tray tooltip that MouseFlock's service isn't running. It SHALL retry about once a second, and show the service's status again as soon as it connects. While not connected it SHALL offer no command that needs the service.

#### Scenario: Service started after the app
- **WHEN** the app is running and the service is stopped, and then the service starts
- **THEN** within 2 seconds the app shows the service's status

#### Scenario: Service stops
- **WHEN** the service stops while the app is connected
- **THEN** the app says the service isn't running, and *Add a PC*, *Remove* and *Forget layout* are unavailable

### Requirement: Tray icon
While the app runs it SHALL show a tray icon. The tooltip SHALL say whether the service is reachable, how many members are up, and which member, if any, has this PC's input or controls this PC. Clicking the icon SHALL open the window. Its menu SHALL offer *Open MouseFlock*, *Add a PC*, *Start with Windows* (a tick) and *Quit*.

#### Scenario: Controlling a peer
- **WHEN** this PC's input is on member B
- **THEN** the tooltip names B as the machine being controlled

#### Scenario: Add a PC from the tray
- **WHEN** the user picks *Add a PC* in the tray menu
- **THEN** the window opens on *Add a PC*

### Requirement: One app per user
At most one instance of the app SHALL run per user session. Starting the app again SHALL show the running instance's window and start nothing new.

#### Scenario: Started twice
- **WHEN** the app is running in the tray and the user starts `MouseFlock.exe` again
- **THEN** the existing window opens and only one tray icon is shown
