# agent-link Specification

## Purpose

Connects an agent, the session agent or the winlogon agent, to the service. The agent reports this machine's input and monitors, and the service tells it what to withhold, inject and warp. The two keep their clocks aligned. The session agent also carries the clipboard, and the winlogon agent reports whether the secure desktop has the input. If the link ends, the agent stops withholding input and exits.

## Requirements

### Requirement: The agent connects to the pipe it is given
The session agent SHALL be started as `mouseflock-agent.exe --session --pipe <name>`, and the winlogon agent as `mouseflock-agent.exe --winlogon --pipe <name>`. Each SHALL connect to that pipe, send `Hello` with its role (`1` session agent, `2` winlogon agent), and start capturing input only after both `Hello`s have agreed a version. If it cannot connect, or the handshake fails, it SHALL exit with a nonzero code without having captured anything.

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
- **WHEN** the agent runs without UI access, as in console mode, and Windows blocks an injected key because the foreground window is elevated
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
When the session's clipboard changes, other than by the agent itself, the agent SHALL report the change once it settles. Changes within 100 ms count as one. A copy holding text SHALL be reported as `ClipboardText` followed by `ClipboardChunk` frames. A copy holding no text but an image SHALL be reported, on a link that agreed IPC version 2 or later, as `ClipboardImage` followed by `ClipboardChunk` frames carrying the image as PNG. Otherwise the agent SHALL send `ClipboardNotShared` with why: private, no text or image, or too large.

#### Scenario: Text copied
- **WHEN** the user copies `hello` in Notepad
- **THEN** the service end receives `ClipboardText` with length 5 and chunks that together are `hello`

#### Scenario: Image copied
- **WHEN** the user copies an image with no text format, on a link that agreed version 2
- **THEN** the service end receives `ClipboardImage` and chunks that together are a PNG of that image

#### Scenario: Image copied on a version 1 link
- **WHEN** the user copies an image with no text format, on a link that agreed version 1
- **THEN** the service end receives `ClipboardNotShared` with reason no text or image

#### Scenario: Nothing shareable
- **WHEN** the user copies files in Explorer, which puts neither text nor an image on the clipboard
- **THEN** the service end receives `ClipboardNotShared` with reason no text or image

### Requirement: Private copies are recognised
The agent SHALL report a copy as private, without reading its contents, when the clipboard holds the format `ExcludeClipboardContentFromMonitorProcessing`. The same SHALL apply when `CanIncludeInClipboardHistory` or `CanUploadToCloudClipboard` holds the value 0.

#### Scenario: Password manager copy
- **WHEN** an app copies text together with `CanUploadToCloudClipboard` set to 0
- **THEN** the service end receives `ClipboardNotShared` with reason private, and no `ClipboardText`

#### Scenario: Private image
- **WHEN** an app copies an image together with `ExcludeClipboardContentFromMonitorProcessing`
- **THEN** the service end receives `ClipboardNotShared` with reason private, and no `ClipboardImage`

### Requirement: The service sets the clipboard
On `SetClipboard` followed by `SetClipboardChunk` frames, the agent SHALL replace the session's clipboard with that text as Unicode text. On `SetClipboardImage` followed by `SetClipboardChunk` frames, the agent SHALL decode the PNG and replace the session's clipboard with the image, as PNG and as a bitmap; if the PNG cannot be decoded or exceeds the image limits, it SHALL leave the clipboard unchanged and keep the link up. It SHALL NOT report any change it makes as a copy. If the clipboard cannot be opened within 1 second, the agent SHALL leave it unchanged.

#### Scenario: Text set
- **WHEN** the service sends `SetClipboard` with `world`
- **THEN** pasting in Notepad gives `world`, and the service end receives no clipboard report for it

#### Scenario: Image set
- **WHEN** the service sends `SetClipboardImage` with a valid PNG
- **THEN** pasting in Paint shows the image, and the service end receives no clipboard report for it

#### Scenario: Corrupt image
- **WHEN** the service sends `SetClipboardImage` with bytes that begin with the PNG signature but do not decode
- **THEN** the clipboard is unchanged and the link stays up

### Requirement: Clipboard runs on the agent link
A `ClipboardText` or `SetClipboard` SHALL announce a length of at most 4,194,304 bytes, and the text SHALL be valid UTF-8. A `ClipboardImage` or `SetClipboardImage` SHALL be used only on a link that agreed IPC version 2 or later, SHALL announce a length of at most 33,554,432 bytes, and its bytes SHALL begin with the PNG signature. The `ClipboardChunk` or `SetClipboardChunk` frames after a start message SHALL carry exactly the announced number of bytes, each with at least one byte. A chunk outside a run, a run cut short by another clipboard message, or any breach of these limits SHALL be a protocol error.

#### Scenario: Chunk without a run
- **WHEN** the service end receives `ClipboardChunk` without a `ClipboardText` or `ClipboardImage` before it
- **THEN** it closes the agent link

#### Scenario: Input during a run
- **WHEN** the agent sends input events between the chunks of a `ClipboardText` or `ClipboardImage` run
- **THEN** the service end accepts them and the run completes

#### Scenario: Image run on a version 1 link
- **WHEN** a link agreed IPC version 1 and the agent sends `ClipboardImage`
- **THEN** the service end closes the agent link

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

### Requirement: A file copy is reported as a manifest
When the session's clipboard holds copied files and no shareable text or image, the session agent SHALL report the copy, on a link that agreed IPC version 3 or later, as a file manifest: each entry's path relative to the copy root, whether it is a directory, and each file's size. It SHALL NOT read any file's contents to report the copy. On an earlier link it SHALL report the copy as not shared, no text or image.

#### Scenario: Files copied
- **WHEN** the user copies two files and a folder, on a link that agreed version 3
- **THEN** the service end receives a file manifest listing the files and the folder, with sizes, and no file contents

#### Scenario: Files copied on a version 1 or 2 link
- **WHEN** the user copies files on a link that agreed version 2
- **THEN** the service end receives a not-shared report, not a manifest

### Requirement: The agent serves a file's contents on request
On a request from the service for a file in the copy the agent currently holds as its own, named by its manifest index, the session agent SHALL stream that file's contents to the service. A request for an index outside the held copy, or for a copy it no longer holds, SHALL be answered with nothing. The agent SHALL stream from disk rather than reading the whole file into memory.

#### Scenario: Content requested
- **WHEN** the service asks the agent for file index 2 of the copy it holds
- **THEN** the agent streams that file's bytes to the service

#### Scenario: Stale content request
- **WHEN** the service asks for a file of a copy the agent no longer holds
- **THEN** the agent sends no contents

### Requirement: The service sets a file copy for paste
On a message giving the agent a file manifest to offer, the session agent SHALL place a copy on the clipboard that offers those files for paste without their bytes, using Windows' delayed rendering. When the file manager pastes and asks for a file's contents, the agent SHALL request that file from the service and stream the received bytes to the file manager. The agent SHALL NOT report any copy it places this way. A winlogon agent's link SHALL never carry a file manifest, in either direction.

#### Scenario: Offer placed, paste served
- **WHEN** the service sends the agent a manifest of one file and the user then pastes in Explorer
- **THEN** the agent offers the file, requests its contents from the service on paste, and the file is written, with no copy reported back

#### Scenario: Manifest on a winlogon link
- **WHEN** a manifest is sent on a winlogon agent's link, in either direction
- **THEN** the link is closed with a protocol error

### Requirement: File runs on the agent link
A file manifest and a file content run SHALL be used only on a link that agreed IPC version 3 or later. A content run SHALL carry one file's bytes, in file chunk frames of its own whose total matches the length its start announced, which MAY exceed the text and image limits. The side sending a file's bytes SHALL keep at most 1 MiB of them unacknowledged by the other side. A chunk outside a run, a run cut short, more than 1 MiB unacknowledged, or a file manifest or content message on a version 1 or 2 link SHALL be a protocol error.

#### Scenario: Content run on a version 2 link
- **WHEN** a link agreed IPC version 2 and a file manifest or content message is sent
- **THEN** the receiver closes the link

#### Scenario: Input during a content run
- **WHEN** input events arrive between the chunks of a file content run
- **THEN** the receiver accepts them and the run completes

#### Scenario: A copy during a content run
- **WHEN** the session agent reports a text copy while it is serving a file
- **THEN** the service end receives the copy and the file run continues

#### Scenario: Past the window
- **WHEN** the agent sends more than 1 MiB of a file's bytes that the service has not acknowledged
- **THEN** the service end closes the agent link
