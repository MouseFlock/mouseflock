## MODIFIED Requirements

### Requirement: Service and console modes
`mouseflock-service.exe` started by the Service Control Manager SHALL run as the service named `MouseFlock`. Started with `--console`, it SHALL run as the current user in the current session. Console mode SHALL keep its data under the user's local application data, or in the folder given with `--data`, and SHALL never touch the service's data folder. `--port` SHALL set its UDP port. If the port is taken, it SHALL exit with an error naming the port.

#### Scenario: Console mode's own folder
- **WHEN** the service is registered and a user starts `mouseflock-service --console`
- **THEN** console mode uses a data folder under that user's profile and leaves the service's files untouched

#### Scenario: Two console instances
- **WHEN** two console instances run on one machine with different `--data` folders and ports
- **THEN** each keeps its own identity and trust store, and each finds the other through discovery

#### Scenario: Port taken
- **WHEN** console mode starts on a port another process holds
- **THEN** it exits with a nonzero code and an error naming the port
