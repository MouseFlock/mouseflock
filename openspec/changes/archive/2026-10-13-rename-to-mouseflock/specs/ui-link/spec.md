## MODIFIED Requirements

### Requirement: The UI pipe
The service SHALL serve UIs on the pipe `\\.\pipe\mouseflock-ui`, created when the service starts as the first instance of that name, refusing remote clients, with at most 8 UIs connected at once. Its ACL SHALL admit only SYSTEM and interactive users. If the name is already taken, the service SHALL log it and retry every 5 seconds.

#### Scenario: Ninth UI
- **WHEN** 8 UIs are connected and a ninth tries to connect
- **THEN** the ninth cannot connect until one of the 8 disconnects

#### Scenario: Name taken at start
- **WHEN** another process holds `\\.\pipe\mouseflock-ui` when the service starts
- **THEN** the service logs it, keeps running without a UI pipe, and creates the pipe once the name is free
