# Spec Delta

## MODIFIED Requirements

### Requirement: Service commands are carried out in order
The agent SHALL carry out `SetCapture`, `Inject` and `Warp` from the service in the order they arrive. A command that fails locally, such as an injection the OS blocks, SHALL NOT end the link.

#### Scenario: Withhold, then warp
- **WHEN** the service sends `SetCapture` with withhold all, then `Warp` to (100, 200)
- **THEN** the agent withholds physical input and moves the cursor to (100, 200)

#### Scenario: Blocked injection
- **WHEN** the agent runs without UI access, as in console mode, and Windows blocks an injected key because the foreground window is elevated
- **THEN** the link stays up and later commands are still carried out
