# Spec Delta

## ADDED Requirements

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
