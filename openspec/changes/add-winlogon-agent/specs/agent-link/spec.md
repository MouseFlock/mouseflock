# Spec Delta

## MODIFIED Requirements

### Requirement: The agent connects to the pipe it is given
The session agent SHALL be started as `deskhop-agent.exe --session --pipe <name>`, and the winlogon agent as `deskhop-agent.exe --winlogon --pipe <name>`. Each SHALL connect to that pipe, send `Hello` with its role (`1` session agent, `2` winlogon agent), and start capturing input only after both `Hello`s have agreed a version. If it cannot connect, or the handshake fails, it SHALL exit with a nonzero code without having captured anything.

#### Scenario: No pipe
- **WHEN** the agent is started with a pipe name that no one has created
- **THEN** it exits with a nonzero code and never installs its hooks

#### Scenario: Handshake then capture
- **WHEN** the service end answers the agent's `Hello`
- **THEN** the agent starts capture and its first report is the screen

#### Scenario: Winlogon agent's role
- **WHEN** an agent started with `--winlogon --pipe <name>` connects
- **THEN** its `Hello` carries role `2`

## ADDED Requirements

### Requirement: Each agent pipe accepts only its agent's role
The service end of an agent link SHALL accept only the role of the agent the service launched on that pipe: `1` on a session agent's pipe, `2` on a winlogon agent's pipe. Any other role SHALL close the connection.

#### Scenario: Session agent on a winlogon pipe
- **WHEN** a client sends `Hello` with role `1` on a pipe the service created for a winlogon agent
- **THEN** the service end closes the connection

### Requirement: The winlogon agent reports the input desktop
The winlogon agent SHALL send `InputDesktop` right after its first `Screen`, saying whether its own desktop is the input desktop. It SHALL send it again within 100 ms of each change. A session agent SHALL never send it, and the service end SHALL treat `InputDesktop` on a session agent's link as a protocol error.

#### Scenario: First reports
- **WHEN** a winlogon agent's handshake completes
- **THEN** its first report is the screen and its second is `InputDesktop`

#### Scenario: Locking the machine
- **WHEN** the winlogon agent runs on the `Winlogon` desktop and the user presses Win+L
- **THEN** it reports that its desktop is the input desktop within 100 ms

#### Scenario: Started on the user's desktop
- **WHEN** an agent started with `--winlogon --pipe <name>` runs on the user's own desktop, as in a test
- **THEN** it reports that its desktop is the input desktop

### Requirement: The winlogon agent has no clipboard
The winlogon agent SHALL neither watch nor set the clipboard. The service end SHALL treat `ClipboardText`, `ClipboardChunk` or `ClipboardNotShared` on a winlogon agent's link as a protocol error, and SHALL refuse to send clipboard text on one.

#### Scenario: Copy while the winlogon agent runs
- **WHEN** a winlogon agent started on the user's desktop is linked and text is copied there
- **THEN** the winlogon agent reports nothing about it

#### Scenario: Clipboard message from a winlogon agent
- **WHEN** a client linked with role `2` sends `ClipboardNotShared`
- **THEN** the service end closes the connection
