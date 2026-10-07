# Spec Delta

## Purpose

Keeps DeskHop working when Windows shows its secure desktop: UAC prompts, the lock screen and the logon screen. It decides which of this machine's agents carries input at any moment, so the cursor can cross onto, and type into, whatever the user would see at the keyboard.

## ADDED Requirements

### Requirement: The agent on the input desktop is active
The service SHALL treat the winlogon agent as the active agent while its latest report says its desktop is the input desktop. Otherwise it SHALL treat the session agent as active, including while no winlogon agent is linked. A change of active agent SHALL take effect when its report arrives.

#### Scenario: UAC prompt
- **WHEN** both agents are linked and a UAC prompt appears on the secure desktop
- **THEN** the winlogon agent becomes active once it reports the change

#### Scenario: Prompt answered
- **WHEN** the UAC prompt is closed and the winlogon agent reports that its desktop is no longer the input desktop
- **THEN** the session agent is active again

#### Scenario: Winlogon agent lost
- **WHEN** the winlogon agent is active and its link ends
- **THEN** the session agent is active

### Requirement: Injection and warps go to the active agent
The service SHALL send each injection and each cursor warp only to the active agent. If the active agent has no link, it SHALL drop them.

#### Scenario: Typing into a UAC prompt
- **WHEN** A controls B and a UAC prompt appears on B
- **THEN** keys typed on A reach B's winlogon agent and not B's session agent

#### Scenario: Unlocking a controlled machine
- **WHEN** A controls B, B is locked, and the user types B's password on A and presses Enter
- **THEN** B unlocks, and later keys reach B's session agent

#### Scenario: Logon after boot
- **WHEN** B has just started and shows its logon screen, and the user crosses from A onto B
- **THEN** B's winlogon agent receives the warp and the user's injected movement and keys

### Requirement: Every agent gets the capture mode
The service SHALL send each capture mode decision to every linked agent, active or not, so physical input is withheld or passed the same way on either desktop.

#### Scenario: Controlling machine shows a UAC prompt
- **WHEN** the user's input has focus on B and a UAC prompt appears on A
- **THEN** A's physical keyboard and mouse stay withheld on A's secure desktop, and the user's input keeps reaching B

#### Scenario: Agents link at different times
- **WHEN** focus is forwarding to B and the winlogon agent links after the session agent
- **THEN** the winlogon agent's first command withholds all input

### Requirement: Input and the screen come from the active agent
The service SHALL give the engine input only from the active agent, and drop input from an inactive agent. The engine SHALL use the screen last reported by the active agent. When the active agent changes and its last screen differs from the one the engine has, the service SHALL give the engine that screen.

#### Scenario: Input from the inactive agent
- **WHEN** the winlogon agent is active and the session agent reports a key press
- **THEN** the engine does not receive it

#### Scenario: Only the winlogon agent at boot
- **WHEN** no user is logged on, so only the winlogon agent is linked, and it reports the screen
- **THEN** the engine uses that screen, and the cursor can cross out through its edges

### Requirement: Losing the active agent releases its input
When the active agent's link ends, the service SHALL tell the engine that local input is lost, as it does when the only agent's link ends. When an inactive agent's link ends, the service SHALL NOT tell the engine anything.

#### Scenario: Winlogon agent dies on the lock screen
- **WHEN** A's input has focus on B, A is locked, and A's winlogon agent's link ends
- **THEN** A's engine is told local input is lost, and A's focus returns to A

#### Scenario: Session agent restarts during a UAC prompt
- **WHEN** the winlogon agent is active and the session agent's link ends
- **THEN** focus does not change

### Requirement: No clipboard on the secure desktop
The service SHALL never send clipboard text to the winlogon agent. Copied text that this machine fetches while the secure desktop is the input desktop SHALL go to the session agent if one is linked, and SHALL otherwise be dropped.

#### Scenario: Cursor arrives at a locked machine
- **WHEN** B is locked, A holds the newest copy, and the cursor crosses from A to B
- **THEN** B's session agent sets the text on the user's clipboard, and B's winlogon agent receives no clipboard message
