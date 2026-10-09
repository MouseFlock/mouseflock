# Spec Delta

## MODIFIED Requirements

### Requirement: Launched as the session user
The service SHALL launch the session agent with the logged-on user's own token, on that session's interactive desktop, without a window. The agent executable SHALL be the one in the service executable's own folder. Every launch SHALL go through a single launch function. In console mode the agent SHALL run as the console mode's own user.

#### Scenario: Agent's account
- **WHEN** the registered service launches an agent for a standard user
- **THEN** the agent runs as that user, not as SYSTEM and not elevated, and its token holds no privilege that the user's own unelevated token lacks

## ADDED Requirements

### Requirement: The session agent has UI access
The registered service SHALL launch the session agent with UI access, so that its injection and hooks work with the user's elevated windows as they do with the user's other windows. It SHALL do so without the agent being code-signed. In console mode, the agent SHALL run without UI access, and the service SHALL log that once.

#### Scenario: Registered service
- **WHEN** the registered service launches the session agent for a logged-on user
- **THEN** the agent's token has UI access, and the log records it for that launch

#### Scenario: Console mode
- **WHEN** the service runs with `--console` and launches the session agent
- **THEN** the agent has no UI access, and the log says once that elevated windows need the registered service

### Requirement: The session agent is shielded from the user's programs
While the session agent has UI access, the session user's unelevated processes SHALL NOT be able to write its memory, create threads in it, change its security, or send messages to its windows, except the setting-change notifications Windows broadcasts. They MAY end it, as they may end any of the user's processes; the service then launches a new one under the restart rule.

#### Scenario: Unelevated program tampers with the agent
- **WHEN** an unelevated program running as the session user opens the session agent's process to write its memory or create a thread
- **THEN** Windows denies access

#### Scenario: Unelevated program messages the agent's window
- **WHEN** an unelevated program running as the session user sends an application-defined message to one of the session agent's hidden windows
- **THEN** Windows does not deliver it

#### Scenario: Scaling changed in Settings
- **WHEN** the user changes a monitor's scaling in Settings while the session agent has UI access
- **THEN** the agent reports the new screen, as it does without UI access

#### Scenario: Agent ended from Task Manager
- **WHEN** the user ends the session agent in Task Manager
- **THEN** a new session agent with UI access is linked within 2 seconds

### Requirement: No UI access without the shield
If the service cannot give the session agent both UI access and the shield, it SHALL launch the agent with neither, as the user's plain token, and SHALL log once per service run why UI access is missing. Input then works with every window except elevated ones.

#### Scenario: Windows refuses UI access
- **WHEN** Windows refuses to give the agent's token UI access
- **THEN** the agent runs without UI access and is linked as usual, and the log says once that elevated windows are unavailable and why

#### Scenario: Shield cannot be applied
- **WHEN** the token can be given UI access but its integrity level cannot be raised
- **THEN** the agent runs with neither, and the log says why
