# ui-link Specification

## Purpose

Connects the DeskHop UI to the service: the service tells every UI what this machine and its desk look like and what just happened, and a UI asks the service to pair, join, remove a machine, or take back a learned side.

## Requirements

### Requirement: The UI pipe
The service SHALL serve UIs on the pipe `\\.\pipe\deskhop-ui`, created when the service starts as the first instance of that name, refusing remote clients, with at most 8 UIs connected at once. Its ACL SHALL admit only SYSTEM and interactive users. If the name is already taken, the service SHALL log it and retry every 5 seconds.

#### Scenario: Ninth UI
- **WHEN** 8 UIs are connected and a ninth tries to connect
- **THEN** the ninth cannot connect until one of the 8 disconnects

#### Scenario: Name taken at start
- **WHEN** another process holds `\\.\pipe\deskhop-ui` when the service starts
- **THEN** the service logs it, keeps running without a UI pipe, and creates the pipe once the name is free

### Requirement: Only a UI at the console
The service SHALL accept a UI only if the connecting process runs in the active console session, and only if its `Hello` names role `3` (UI). Any other client SHALL be closed before the service sends it anything but `Hello`. In console mode, the pipe SHALL admit only the user running console mode.

#### Scenario: UI in a remote session
- **WHEN** a UI running in an RDP session connects while another user is at the console
- **THEN** the service closes it without sending a `Status`

#### Scenario: Agent on the UI pipe
- **WHEN** a client connects to the UI pipe and sends `Hello` with role `1`
- **THEN** the service closes it

### Requirement: Status on connect and on change
After the `Hello` exchange, the service SHALL send the UI a `Status` at once. It SHALL send a new `Status` whenever any field in it changes, at most once every 100 ms, with the latest values. Each `Status` SHALL be complete, so a UI never needs an earlier one.

#### Scenario: Peer goes down
- **WHEN** a UI is connected and peer B goes down
- **THEN** within 200 ms the UI receives a `Status` listing B as down

#### Scenario: Burst of changes
- **WHEN** 50 changes happen within 100 ms
- **THEN** a connected UI receives at most two `Status` messages for them, the last with the final values

### Requirement: Status contents
A `Status` SHALL carry:
- this machine's name and key;
- whether the session agent is linked;
- pairing: closed, or open with its code and the milliseconds left;
- focus: local, on a member, or controlled by a member, naming the member's key;
- every member with its name, key, and whether it is up.

#### Scenario: Pairing open
- **WHEN** a UI sends `OpenPairing` and pairing opens
- **THEN** the next `Status` shows pairing open with the code and about 600,000 ms left

#### Scenario: After pairing
- **WHEN** B joins this machine
- **THEN** the next `Status` shows pairing closed and lists B as a member

### Requirement: UI commands
The service SHALL carry out these UI commands as the same actions console mode offers:
- `OpenPairing` and `ClosePairing` open and close pairing mode;
- `Join` pairs using the code as the user typed it;
- `RemovePeer` removes the named member;
- `Undo` and `ForgetLayout` act on the learned layout.

A `RemovePeer` for a key that is not a member SHALL change nothing.

#### Scenario: Join from the UI
- **WHEN** A shows code `C` and a UI on B sends `Join` with `C`
- **THEN** A and B pair, and B's UI receives `Paired` naming A

#### Scenario: Remove an unknown key
- **WHEN** a UI sends `RemovePeer` with a key that is not a member
- **THEN** nothing changes and the link stays up

### Requirement: Notices
The service SHALL send these notices to every connected UI when they happen:
- `Paired`, with the member's name and key;
- `JoinFailed`, with its reason: wrong code, not ready, protocol error, desk full, connection lost, unreachable, a mistyped code, or a malformed code;
- `PairingClosed`, with its reason: a machine joined, time ran out, closed by the user, or too many wrong codes;
- `PairingNotOpened`, when there is no network;
- `Learned`, with the member, the side, and the milliseconds left to Undo.

#### Scenario: Mistyped code
- **WHEN** a UI sends `Join` with a code whose check digit is wrong
- **THEN** that UI's service sends `JoinFailed` with reason mistyped code, and no connection is attempted

#### Scenario: Learned side
- **WHEN** this machine learns that B is to its right
- **THEN** every connected UI receives `Learned` naming B, side right, and 10,000 ms

### Requirement: A slow or hostile UI cannot block the service
The service SHALL send to each UI without waiting for it to read. A UI with more than 256 unsent messages SHALL be disconnected. The service SHALL read each UI holding at most one frame of unread data. A UI disconnecting or breaking the protocol SHALL affect no other UI and nothing else in the service.

#### Scenario: UI stops reading
- **WHEN** a UI stops reading its pipe while the service keeps sending it `Status`
- **THEN** the service disconnects that UI, and other UIs, the agent and peers carry on

#### Scenario: Malformed command
- **WHEN** a UI sends a `Join` whose code length field exceeds 32
- **THEN** the service closes that UI's link and nothing else changes

### Requirement: UI message layouts
Every UI message SHALL have the layout `docs/protocol.md` assigns it. Names SHALL be 1 to 63 bytes of UTF-8, codes 1 to 32 bytes of printable ASCII, and a `Status` SHALL list at most 256 members. A message that breaks a layout or a limit, or a UI message received on an agent link, or an agent message on a UI link, SHALL be a protocol error.

#### Scenario: UI command on an agent pipe
- **WHEN** a session agent sends `OpenPairing` on its agent link
- **THEN** the service closes the agent link
