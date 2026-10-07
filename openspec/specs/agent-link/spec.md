# agent-link Specification

## Purpose

Connects an agent, the session agent or the winlogon agent, to the service. The agent reports this machine's input and monitors, and the service tells it what to withhold, inject and warp. The two keep their clocks aligned. The session agent also carries the clipboard, and the winlogon agent reports whether the secure desktop has the input. If the link ends, the agent stops withholding input and exits.

## Requirements

### Requirement: The agent connects to the pipe it is given
The session agent SHALL be started as `deskhop-agent.exe --session --pipe <name>`, and the winlogon agent as `deskhop-agent.exe --winlogon --pipe <name>`. Each SHALL connect to that pipe, send `Hello` with its role (`1` session agent, `2` winlogon agent), and start capturing input only after both `Hello`s have agreed a version. If it cannot connect, or the handshake fails, it SHALL exit with a nonzero code without having captured anything.

#### Scenario: No pipe
- **WHEN** the agent is started with a pipe name that no one has created
- **THEN** it exits with a nonzero code and never installs its hooks

#### Scenario: Handshake then capture
- **WHEN** the service end answers the agent's `Hello`
- **THEN** the agent starts capture and its first report is the screen

#### Scenario: Winlogon agent's role
- **WHEN** an agent started with `--winlogon --pipe <name>` connects
- **THEN** its `Hello` carries role `2`

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

### Requirement: Clipboard copies are reported
When the session's clipboard changes, other than by the agent itself, the agent SHALL report the change once it settles. Changes within 100 ms count as one. The report SHALL be the copied text, as `ClipboardText` followed by `ClipboardChunk` frames, or `ClipboardNotShared` with why: private, no text, or text over 4 MiB.

#### Scenario: Text copied
- **WHEN** the user copies `hello` in Notepad
- **THEN** the service end receives `ClipboardText` with length 5 and chunks that together are `hello`

#### Scenario: Image copied
- **WHEN** the user copies an image with no text format
- **THEN** the service end receives `ClipboardNotShared` with reason no text

### Requirement: Private copies are recognised
The agent SHALL report a copy as private, without reading its text, when the clipboard holds the format `ExcludeClipboardContentFromMonitorProcessing`. The same SHALL apply when `CanIncludeInClipboardHistory` or `CanUploadToCloudClipboard` holds the value 0.

#### Scenario: Password manager copy
- **WHEN** an app copies text together with `CanUploadToCloudClipboard` set to 0
- **THEN** the service end receives `ClipboardNotShared` with reason private, and no `ClipboardText`

### Requirement: The service sets the clipboard
On `SetClipboard` followed by `SetClipboardChunk` frames, the agent SHALL replace the session's clipboard with that text as Unicode text. It SHALL NOT report that change as a copy. If the clipboard cannot be opened within 1 second, the agent SHALL leave it unchanged.

#### Scenario: Text set
- **WHEN** the service sends `SetClipboard` with `world`
- **THEN** pasting in Notepad gives `world`, and the service end receives no clipboard report for it

### Requirement: Clipboard runs on the agent link
A `ClipboardText` or `SetClipboard` SHALL announce a length of at most 4,194,304 bytes. The `ClipboardChunk` or `SetClipboardChunk` frames after it SHALL carry exactly that many bytes, each with at least one byte, and the text SHALL be valid UTF-8. A chunk outside a run, a run cut short by another clipboard message, or any breach of these limits SHALL be a protocol error.

#### Scenario: Chunk without a run
- **WHEN** the service end receives `ClipboardChunk` without a `ClipboardText` before it
- **THEN** it closes the agent link

#### Scenario: Input during a run
- **WHEN** the agent sends input events between the chunks of a `ClipboardText` run
- **THEN** the service end accepts them and the run completes

### Requirement: Each agent pipe accepts only its agent's role
The service end of an agent link SHALL accept only the role of the agent the service launched on that pipe: `1` on a session agent's pipe, `2` on a winlogon agent's pipe. Any other role SHALL close the connection.

#### Scenario: Session agent on a winlogon pipe
- **WHEN** a client sends `Hello` with role `1` on a pipe the service created for a winlogon agent
- **THEN** the service end closes the connection

### Requirement: The winlogon agent reports the input desktop
The winlogon agent SHALL send `InputDesktop` right after its first `Screen`, saying whether its own desktop is the input desktop. It SHALL send it again within 100 ms of each change. A session agent SHALL never send it, and the service end SHALL treat `InputDesktop` on a session agent's link as a protocol error.

#### Scenario: First reports
- **WHEN** a winlogon agent's handshake completes
- **THEN** its first report is the screen and its second is `InputDesktop`

#### Scenario: Locking the machine
- **WHEN** the winlogon agent runs on the `Winlogon` desktop and the user presses Win+L
- **THEN** it reports that its desktop is the input desktop within 100 ms

#### Scenario: Started on the user's desktop
- **WHEN** an agent started with `--winlogon --pipe <name>` runs on the user's own desktop, as in a test
- **THEN** it reports that its desktop is the input desktop

### Requirement: The winlogon agent has no clipboard
The winlogon agent SHALL neither watch nor set the clipboard. The service end SHALL treat `ClipboardText`, `ClipboardChunk` or `ClipboardNotShared` on a winlogon agent's link as a protocol error, and SHALL refuse to send clipboard text on one.

#### Scenario: Copy while the winlogon agent runs
- **WHEN** a winlogon agent started on the user's desktop is linked and text is copied there
- **THEN** the winlogon agent reports nothing about it

#### Scenario: Clipboard message from a winlogon agent
- **WHEN** a client linked with role `2` sends `ClipboardNotShared`
- **THEN** the service end closes the connection
