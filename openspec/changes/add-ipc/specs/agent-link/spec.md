# Spec Delta

## Purpose

Connects a session agent to the service: the agent reports this machine's input and monitors, the service tells it what to withhold, inject and warp, and the two keep their clocks aligned. If the link ends, the agent stops withholding input and exits.

## ADDED Requirements

### Requirement: The agent connects to the pipe it is given
The session agent SHALL be started as `deskhop-agent.exe --session --pipe <name>` and SHALL connect to that pipe, send `Hello` with role `1`, and start capturing input only after both `Hello`s have agreed a version. If it cannot connect, or the handshake fails, it SHALL exit with a nonzero code without having captured anything.

#### Scenario: No pipe
- **WHEN** the agent is started with a pipe name that no one has created
- **THEN** it exits with a nonzero code and never installs its hooks

#### Scenario: Handshake then capture
- **WHEN** the service end answers the agent's `Hello`
- **THEN** the agent starts capture and its first report is the screen

### Requirement: Captured input is reported in order
The agent SHALL send every captured event to the service as an `Input` message carrying the event time, origin, and the key, button, wheel or motion, with the cursor position for motion, in the order it captured them.

#### Scenario: Key press reaches the service
- **WHEN** the user presses and releases a key on the agent's desktop
- **THEN** the service end receives two `Input` messages for that key, press then release, each marked physical

### Requirement: Screen reports
The agent SHALL send a `Screen` message each time capture reports the monitors. The service end SHALL treat as a protocol error a screen with more than 32 monitors, a monitor identity longer than 256 bytes or not UTF-8, a width or height below 1, or a DPI outside 48 to 960.

#### Scenario: Monitor added
- **WHEN** a second monitor is connected while the link is up
- **THEN** the service end receives a `Screen` message listing both monitors

#### Scenario: Impossible screen
- **WHEN** an agent sends a `Screen` listing a monitor 0 pixels wide
- **THEN** the service end closes the connection

### Requirement: Service commands are carried out in order
The agent SHALL carry out `SetCapture`, `Inject` and `Warp` from the service in the order they arrive. A command that fails locally, such as an injection the OS blocks, SHALL NOT end the link.

#### Scenario: Withhold, then warp
- **WHEN** the service sends `SetCapture` with withhold all, then `Warp` to (100, 200)
- **THEN** the agent withholds physical input and moves the cursor to (100, 200)

#### Scenario: Blocked injection
- **WHEN** Windows blocks an injected key because the foreground window is elevated
- **THEN** the link stays up and later commands are still carried out

### Requirement: Clock alignment
The service end SHALL send `Ping` carrying its own clock reading when the link comes up and every 10 seconds after. The agent SHALL answer each `Ping` at once with `Pong` carrying the service's reading and its own. The service end SHALL estimate the agent clock's offset from the exchange with the shortest round trip seen so far, and SHALL report every agent event time converted to the service's clock. Until the first `Pong`, it SHALL hold the agent's events rather than report them with an unknown offset.

#### Scenario: Times converted
- **WHEN** the shortest exchange so far was sent at service time 1,000, carried agent time 51,002, and was answered at service time 1,004
- **THEN** an agent event stamped 51,500 is reported at service time 1,500

#### Scenario: Events before the first Pong
- **WHEN** the agent reports input before its first `Pong` reaches the service
- **THEN** the service end reports those events, in order and converted, once the `Pong` arrives

### Requirement: The link ending ends capture
When the link closes for any reason (the service ends it, the pipe breaks, or either end sees a protocol error), the agent SHALL return capture to pass all at once and exit.

#### Scenario: Service killed while withholding
- **WHEN** the service sets withhold all and its process is then ended
- **THEN** the agent stops withholding within 100 ms and exits, and the user's keyboard and mouse work normally

### Requirement: A slow or hostile agent cannot block the service
The service end SHALL read the agent's messages without waiting on whoever consumes them, holding at most one frame of unread data, and SHALL deliver decoded messages through a channel the service drains. Sending a command to the agent SHALL NOT wait for the agent to read it.

#### Scenario: Agent stops reading
- **WHEN** an agent stops reading its pipe and the service sends 1,000 `Inject` commands
- **THEN** every send returns without waiting, and the service end keeps delivering that agent's messages
