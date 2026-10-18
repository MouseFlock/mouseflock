# ipc-protocol Specification

## Purpose

Defines the envelope of the named-pipe protocol between the MouseFlock service, its agents and the UI: how bytes become messages, how the two ends agree on a version and say what they are, and how decoding stays safe when the less trusted end sends anything at all.

## Requirements

### Requirement: IPC frame layout
Every message on a pipe SHALL be one frame: the payload length as an unsigned 32-bit little-endian integer, the message type as an unsigned 16-bit little-endian integer, and the payload. The payload SHALL NOT exceed 65,536 bytes, and a receiver SHALL reject a larger length as a protocol error from the header alone, before reading the payload.

#### Scenario: Frame bytes
- **WHEN** an end sends message type `0x0003` with the 1-byte payload `01`
- **THEN** the bytes on the pipe are `01 00 00 00 03 00 01`

#### Scenario: Oversized length
- **WHEN** a receiver reads a header whose payload length is 65,537
- **THEN** it reports a protocol error without reading the payload

### Requirement: Hello with role and version
Each end SHALL send `Hello` as its first frame without waiting for the other's. `Hello` SHALL carry the lowest and highest IPC version the sender speaks, as unsigned 16-bit little-endian integers, and the sender's role: `1` session agent, `2` winlogon agent, `3` UI, or `4` service. Both ends SHALL use the highest version both ranges contain. An end SHALL close the connection when the ranges do not overlap, when the first frame is not `Hello`, when a second `Hello` arrives, or when the other end's role is not one it expects on that pipe.

#### Scenario: Agent and service agree
- **WHEN** the service speaks IPC versions 1 to 2 and a session agent speaks 1 to 1
- **THEN** both use version 1

#### Scenario: Wrong role on an agent pipe
- **WHEN** the service end of an agent link receives a `Hello` with role `3` (UI)
- **THEN** it closes the connection

#### Scenario: Versions do not overlap
- **WHEN** one end speaks versions 2 to 3 and the other speaks 1 to 1
- **THEN** the connection is closed

### Requirement: IPC message registry
Each IPC message type SHALL be a 16-bit number assigned in `docs/protocol.md`, together with the direction it may travel and the IPC version that introduced it. Type `0x0000` SHALL be reserved. A message whose type is not defined in the agreed version, or that travels in a direction its type does not allow, SHALL be a protocol error.

#### Scenario: Agent sends a service command
- **WHEN** a session agent sends `Inject` to the service
- **THEN** the service treats it as a protocol error and closes the connection

#### Scenario: Unknown type
- **WHEN** an end receives message type `0x0042` after agreeing version 1
- **THEN** it closes the connection

### Requirement: Strict IPC payloads
A payload that is shorter or longer than its layout, holds a code its layout does not define, or breaks a limit its message sets SHALL be a protocol error, and the receiver SHALL close the connection. `Hello` SHALL be the only exception: bytes after its defined fields SHALL be ignored, so that later versions can extend it.

#### Scenario: Trailing byte
- **WHEN** the service receives a `Pong` payload one byte longer than its layout
- **THEN** it closes the connection

#### Scenario: Newer Hello
- **WHEN** an end receives a `Hello` with valid version and role fields followed by 6 more bytes
- **THEN** it ignores the extra bytes

### Requirement: IPC decoding is total and bounded
Decoding frames and payloads SHALL NOT panic, abort or loop forever on any input, and SHALL NOT allocate memory based on a length the other end claims. Memory held for one connection while decoding SHALL be bounded by the frame header plus the 65,536-byte frame limit.

#### Scenario: Arbitrary bytes from an agent
- **WHEN** the service end is fed any sequence of bytes
- **THEN** each step yields a message, a need for more input, or a protocol error, and never panics

### Requirement: IPC version range of this release
This release SHALL speak IPC versions 1 to 3. Version 2 SHALL differ from version 1 only by adding `ClipboardImage` (`0x001A`, session agent to service) and `SetClipboardImage` (`0x0029`, service to session agent). Version 3 SHALL differ from version 2 only by adding, for file copies, the manifest and per-file content messages in each direction. A link SHALL behave exactly as the older release does whenever it agrees a version below the one that defines a feature.

#### Scenario: Hello sent by a session agent of this release
- **WHEN** a session agent of this release sends `Hello`
- **THEN** the payload is `01 00 03 00 01`

#### Scenario: Agent from the previous release
- **WHEN** the service of this release links with a session agent whose `Hello` advertises IPC versions 1 to 1, as during an upgrade
- **THEN** the link agrees version 1, and input and text clipboard work, with no image or file messages in either direction

#### Scenario: Agent that understands images but not files
- **WHEN** the service of this release links with a session agent whose `Hello` advertises IPC versions 1 to 2, as during an upgrade
- **THEN** the link agrees version 2, input, text and image clipboard work, and no file message travels in either direction
