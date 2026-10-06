# Spec Delta

## Purpose

Keeps one session agent running for the user at the console. It launches the agent as that user, protects the pipe it connects through so that only that agent can claim this machine's input, and restarts or ends the agent as sessions change.

## ADDED Requirements

### Requirement: One agent for the console session
While a user is logged on to the active console session, the service SHALL keep exactly one session agent running in that session. While no user is logged on there, it SHALL run none. It SHALL follow logon, logoff and console session switches. In console mode, the agent SHALL run in the current session.

#### Scenario: Boot without logon
- **WHEN** the machine has started and no one has logged on
- **THEN** no session agent runs

#### Scenario: Logon
- **WHEN** a user logs on at the console
- **THEN** a session agent runs in that session within 2 seconds

#### Scenario: Switching users
- **WHEN** a second user is switched to at the console
- **THEN** the first session's agent is ended, and an agent runs in the second user's session

### Requirement: Launched as the session user
The service SHALL launch the session agent with the logged-on user's own token, on that session's interactive desktop, without a window. The agent executable SHALL be the one in the service executable's own folder. Every launch SHALL go through a single launch function. In console mode the agent SHALL run as the console mode's own user.

#### Scenario: Agent's account
- **WHEN** the registered service launches an agent for a standard user
- **THEN** the agent runs as that user at medium integrity, not as SYSTEM

### Requirement: A new pipe for every launch
Before each launch, the service SHALL create a pipe whose name carries at least 128 random bits and is never reused. It SHALL create the pipe as the only instance of that name, refusing to start if the name already exists, and SHALL refuse remote clients. The agent SHALL be given the name on its command line.

#### Scenario: Name already taken
- **WHEN** a process has already created a pipe with the chosen name
- **THEN** the service's creation fails, it does not launch the agent with that name, and it retries with a new name

### Requirement: Only SYSTEM and the session user can open the pipe
The agent pipe's ACL SHALL allow only SYSTEM and the user the agent runs as to open it.

#### Scenario: Another user's process
- **WHEN** a process running as a different user tries to open the agent pipe
- **THEN** Windows denies access

### Requirement: Only the launched process is accepted
When a client connects to the agent pipe, the service SHALL compare the client's process ID with the ID of the process it launched. On a mismatch, it SHALL close the pipe without reading from it, end the launched agent, log it, and treat the launch as failed.

#### Scenario: Another process of the same user connects first
- **WHEN** a different process running as the session user connects to the agent pipe before the agent does
- **THEN** the service closes that connection unread, and no input from it reaches the engine

### Requirement: The link must come up in time
If the launched agent has not connected and finished its handshake within 10 seconds, the service SHALL end that process and treat the launch as failed.

#### Scenario: Agent stuck at start
- **WHEN** a launched agent never connects to its pipe
- **THEN** the service ends it after 10 seconds and launches again after the backoff

### Requirement: Restart with backoff
When the agent exits or its link ends while its session still has a logged-on user, the service SHALL launch a new one. It SHALL wait 0.5 seconds after the first failure and double the wait after each further failure, up to 30 seconds. The wait SHALL return to 0.5 seconds once a link has stayed up for 30 seconds.

#### Scenario: Agent killed
- **WHEN** the agent process is killed while its user is logged on
- **THEN** a new agent is linked within 2 seconds

#### Scenario: Agent keeps failing
- **WHEN** every launch fails
- **THEN** the service waits 0.5, 1, 2, 4, 8, 16 and then 30 seconds between launches

### Requirement: Ending an agent
When the agent's session logs off or stops being the console session, or the service stops, the service SHALL end the agent's link. If the agent has not exited 2 seconds later, the service SHALL terminate it.

#### Scenario: Logoff
- **WHEN** the user logs off
- **THEN** the session agent has exited before the session ends, and the service runs no agent until the next logon
