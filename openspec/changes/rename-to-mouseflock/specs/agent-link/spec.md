## MODIFIED Requirements

### Requirement: The agent connects to the pipe it is given
The session agent SHALL be started as `mouseflock-agent.exe --session --pipe <name>`, and the winlogon agent as `mouseflock-agent.exe --winlogon --pipe <name>`. Each SHALL connect to that pipe, send `Hello` with its role (`1` session agent, `2` winlogon agent), and start capturing input only after both `Hello`s have agreed a version. If it cannot connect, or the handshake fails, it SHALL exit with a nonzero code without having captured anything.

#### Scenario: No pipe
- **WHEN** the agent is started with a pipe name that no one has created
- **THEN** it exits with a nonzero code and never installs its hooks

#### Scenario: Handshake then capture
- **WHEN** the service end answers the agent's `Hello`
- **THEN** the agent starts capture and its first report is the screen

#### Scenario: Winlogon agent's role
- **WHEN** an agent started with `--winlogon --pipe <name>` connects
- **THEN** its `Hello` carries role `2`
