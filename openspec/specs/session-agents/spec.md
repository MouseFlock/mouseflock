# session-agents Specification

## Purpose

Keeps one session agent running for the user at the console, and one winlogon agent on the console session's secure desktop. It launches each agent as the right account, and protects the pipe each one connects through so that only that agent can claim this machine's input. It restarts or ends each agent as sessions change.

## Requirements

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
- **THEN** the agent runs as that user, not as SYSTEM and not elevated, and its token holds no privilege that the user's own unelevated token lacks

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
- **THEN** the session agent has exited before the session ends, and the service runs no session agent until the next logon

### Requirement: One winlogon agent for the console session
While a session is attached to the console, whether or not a user is logged on to it, the registered service SHALL keep exactly one winlogon agent running in that session. It SHALL follow console session switches. In console mode, the service SHALL run no winlogon agent, and SHALL log once that the winlogon agent needs the registered service.

#### Scenario: Logon screen after boot
- **WHEN** the machine has started, the service is running, and no one has logged on
- **THEN** a winlogon agent runs in the console session within 2 seconds, and no session agent runs

#### Scenario: Logon
- **WHEN** a user logs on at the console
- **THEN** the winlogon agent keeps running, and a session agent starts beside it

#### Scenario: Switching users
- **WHEN** the console switches to another session
- **THEN** the first session's winlogon agent is ended, and a winlogon agent runs in the new console session

#### Scenario: Console mode
- **WHEN** the service runs with `--console`
- **THEN** no winlogon agent is launched, and the log says why once

### Requirement: Winlogon agent launched as SYSTEM on the secure desktop
The service SHALL launch the winlogon agent as SYSTEM, in the console session, on the `winsta0\Winlogon` desktop, without a window. Its token SHALL hold no privilege except `SeChangeNotifyPrivilege`. The executable SHALL be the agent in the service executable's own folder, and the launch SHALL go through the same single launch function as the session agent's.

#### Scenario: Agent's account
- **WHEN** the registered service launches the winlogon agent
- **THEN** the agent runs as SYSTEM at system integrity in the console session, on the `Winlogon` desktop, and its token lists `SeChangeNotifyPrivilege` as its only privilege

### Requirement: Only SYSTEM can open the winlogon agent's pipe
The winlogon agent's pipe SHALL be created like the session agent's: a new name with at least 128 random bits for each launch, as the only instance, refusing remote clients. Its ACL SHALL allow only SYSTEM to open it.

#### Scenario: An administrator's process
- **WHEN** a process running as an administrator, elevated, tries to open the winlogon agent's pipe
- **THEN** Windows denies access

### Requirement: The winlogon agent is supervised like the session agent
The service SHALL supervise the winlogon agent under the session agent's rules, independently of it. Only the launched process is accepted on its pipe. Its link must come up within 10 seconds. It is restarted with the same backoff while its session is still the console session. It is ended, then terminated after 2 seconds, when its session leaves the console or the service stops.

#### Scenario: Winlogon agent killed
- **WHEN** the winlogon agent process is killed while its session is attached to the console
- **THEN** a new winlogon agent is linked within 2 seconds, and the session agent's link is unaffected

#### Scenario: Another process connects first
- **WHEN** a process other than the launched winlogon agent connects to its pipe
- **THEN** the service closes that connection unread, ends the launched agent, and launches again after the backoff

#### Scenario: Service stops
- **WHEN** the service is stopped
- **THEN** both agents' links end, and both agents have exited within 2 seconds

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
