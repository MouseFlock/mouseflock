# wire-protocol Specification

## Purpose

Defines the envelope of the DeskHop peer wire protocol: how bytes on QUIC streams and datagrams become messages, how peers agree on a protocol version, and how decoding stays safe on untrusted input. Every other peer-to-peer capability carries its messages inside this envelope.

## Requirements

### Requirement: Stream frame layout
Every message on a QUIC stream SHALL be sent as one frame: a 6-byte header followed by the payload. The header SHALL be the payload length as an unsigned 32-bit little-endian integer, then the message type as an unsigned 16-bit little-endian integer. The payload length SHALL count only the payload bytes, not the header. The header layout SHALL never change in any protocol version, because it is read before a version is agreed.

#### Scenario: Frame with a payload
- **WHEN** a peer sends message type `0x0001` with a 4-byte payload `01 00 01 00`
- **THEN** the bytes on the stream are `04 00 00 00 01 00 01 00 01 00`

#### Scenario: Frame with an empty payload
- **WHEN** a peer sends a message whose payload is empty
- **THEN** the frame is exactly the 6-byte header with a payload length of 0

### Requirement: Incremental stream decoding
A receiver SHALL decode stream frames from partial input. When fewer bytes than one complete frame are available, the receiver SHALL wait for more bytes and SHALL NOT treat the input as an error or consume any of it. When several frames arrive together, the receiver SHALL decode them in order.

#### Scenario: Header split across reads
- **WHEN** a receiver has 3 bytes of a frame header available
- **THEN** it reports that more input is needed and consumes nothing

#### Scenario: Two frames in one read
- **WHEN** a receiver's input holds two complete frames back to back
- **THEN** it yields the first frame, then the second, each with its own type and payload

### Requirement: Frame size limit
The payload length of a stream frame SHALL NOT exceed 65,536 bytes. A receiver SHALL reject a larger length as a protocol error as soon as it has read the header, before any of the payload arrives. Messages whose data can exceed the limit SHALL be split across several frames by the capability that defines them.

#### Scenario: Oversized length in the header
- **WHEN** a receiver reads a header whose payload length is 65,537
- **THEN** it reports a protocol error without waiting for or buffering the payload

#### Scenario: Frame at the limit
- **WHEN** a receiver reads a frame whose payload length is exactly 65,536 and the full payload arrives
- **THEN** it decodes the frame normally

### Requirement: Datagram layout
Every message sent as a QUIC datagram SHALL be one datagram: the message type as an unsigned 16-bit little-endian integer followed by the payload, with no length field. A datagram SHALL NOT exceed 1,200 bytes including the type. A receiver SHALL treat a datagram shorter than 2 bytes, or longer than 1,200 bytes, as a protocol error.

#### Scenario: Well-formed datagram
- **WHEN** a receiver gets a datagram of 20 bytes whose first two bytes are a known datagram message type
- **THEN** it yields that message type and the remaining 18 bytes as the payload

#### Scenario: Truncated datagram
- **WHEN** a receiver gets a datagram of 1 byte
- **THEN** it reports a protocol error

### Requirement: Message type registry
Each message type SHALL be a 16-bit number assigned in `docs/protocol.md`, together with the channel it may use (stream or datagram) and the protocol version that introduced it. Type `0x0000` SHALL be reserved and never valid. A receiver SHALL treat as a protocol error any message whose type is not defined in the negotiated version, and any message that arrives on a channel its type does not allow. Protocol version 1 SHALL define exactly these message types on streams: `0x0001` `Hello`, `0x0002` `PairPake`, `0x0003` `PairConfirm`, `0x0004` `MemberRecord`, `0x0005` `RecordsDone`, `0x0006` `FocusEnter`, `0x0007` `FocusRefused`, `0x0008` `EdgeExit`, `0x0009` `TakenBack`, `0x000A` `FocusWithdrawn`, `0x000B` `ForgetSide`, `0x000C` `KeyInput`, `0x000D` `ButtonInput`, `0x000E` `WheelInput`, and `0x000F` `SecureAttention`; and this type on datagrams: `0x0010` `Motion`.

#### Scenario: Reserved type
- **WHEN** a receiver decodes a frame with message type `0x0000`
- **THEN** it reports a protocol error

#### Scenario: Unknown type after negotiation
- **WHEN** peers have negotiated version 1 and a receiver decodes a frame with message type `0x0042`
- **THEN** it reports a protocol error

#### Scenario: Stream-only type in a datagram
- **WHEN** a receiver gets a datagram with message type `0x0001` (`Hello`)
- **THEN** it reports a protocol error

#### Scenario: Pairing message in a datagram
- **WHEN** a receiver gets a datagram with message type `0x0004` (`MemberRecord`)
- **THEN** it reports a protocol error

#### Scenario: Motion on a stream
- **WHEN** a receiver decodes a stream frame with message type `0x0010` (`Motion`)
- **THEN** it reports a protocol error

### Requirement: Strict payload length
A receiver SHALL treat as a protocol error any payload that is shorter than its message type's layout requires, or that has bytes left over after the layout is decoded. `Hello` SHALL be the only exception: bytes after its defined fields SHALL be ignored, so that later versions can extend it.

#### Scenario: Trailing bytes on a non-Hello message
- **WHEN** a message type's layout is 8 bytes and a receiver gets a payload of 9 bytes for it
- **THEN** it reports a protocol error

#### Scenario: Hello from a newer peer with extra fields
- **WHEN** a receiver gets a `Hello` payload of 10 bytes whose first 4 bytes are valid
- **THEN** it decodes the version range from the first 4 bytes and ignores the remaining 6

### Requirement: Hello handshake
Each peer SHALL send `Hello` as the first frame on the control stream as soon as the connection is established, without waiting for the other peer's `Hello`. The `Hello` payload SHALL start with the lowest and then the highest protocol version the sender speaks, each an unsigned 16-bit little-endian integer. A receiver SHALL treat as a protocol error: a first frame on the control stream that is not `Hello`; a second `Hello`; a `Hello` on any other stream; a `Hello` whose lowest version is 0 or greater than its highest. Datagrams received before the version is agreed SHALL be dropped without error. A frame other than `Hello` that arrives on any other stream before the version is agreed SHALL be held, not rejected, and checked once the version is agreed, because QUIC does not order data across streams.

#### Scenario: Both peers send Hello at once
- **WHEN** two peers connect and each sends `Hello` immediately
- **THEN** each peer receives the other's `Hello` as the first frame on the control stream, and neither waited for the other before sending

#### Scenario: Message before Hello
- **WHEN** the first frame a receiver decodes on the control stream is not `Hello`
- **THEN** it reports a protocol error

#### Scenario: Invalid version range
- **WHEN** a receiver gets a `Hello` with lowest version 3 and highest version 2
- **THEN** it reports a protocol error

#### Scenario: Stream frame races ahead of Hello
- **WHEN** a frame arrives on a stream other than the control stream before the version has been agreed
- **THEN** the receiver holds it without error and checks it against the agreed version once `Hello` arrives

#### Scenario: Datagram races ahead of Hello
- **WHEN** a datagram arrives before the version has been agreed
- **THEN** the receiver drops it and the connection stays open

### Requirement: Version negotiation
After exchanging `Hello`, both peers SHALL use the highest protocol version that both ranges contain. Both peers SHALL reach the same result from the two ranges without a further message. When the ranges do not overlap, each peer SHALL close the connection with the version-mismatch close reason and SHALL report whether it or the other peer is running the older version.

#### Scenario: Overlapping ranges
- **WHEN** one peer speaks versions 1 to 3 and the other speaks versions 2 to 5
- **THEN** both peers use version 3

#### Scenario: Other peer is older
- **WHEN** this peer speaks versions 4 to 5 and the other peer speaks versions 1 to 2
- **THEN** this peer closes the connection with the version-mismatch reason and reports that the other peer runs the older version

#### Scenario: This peer is older
- **WHEN** this peer speaks versions 1 to 2 and the other peer speaks versions 4 to 5
- **THEN** this peer closes the connection with the version-mismatch reason and reports that it runs the older version itself

### Requirement: Protocol version range of this release
This release SHALL speak protocol version 1 only, advertising a lowest and highest version of 1 in its `Hello`.

#### Scenario: Hello sent by this release
- **WHEN** this release sends `Hello`
- **THEN** the payload is `01 00 01 00`

### Requirement: Close reasons
A peer closing a connection SHALL give one of these QUIC application error codes: `0` normal close, `1` protocol error, `2` version mismatch, `3` wrong pairing code, `4` not ready to pair, `5` not a member. A peer SHALL close with protocol error whenever this specification calls something a protocol error. New close reasons SHALL be added in `docs/protocol.md` with the version that introduced them.

#### Scenario: Protocol error closes the connection
- **WHEN** a receiver reports a protocol error on any stream or datagram
- **THEN** the peer closes the connection with code `1`

#### Scenario: Version mismatch closes the connection
- **WHEN** version negotiation finds no overlap
- **THEN** the peer closes the connection with code `2`

#### Scenario: Pairing close codes
- **WHEN** a receiver reads close code `3` or `4`
- **THEN** it reports a wrong pairing code or a machine not ready to pair, respectively

#### Scenario: Not a member
- **WHEN** a receiver reads close code `5`
- **THEN** it reports that the other machine does not count it as a member of its desk

### Requirement: Decoding is total and bounded
Decoding frames, datagrams, and message payloads SHALL NOT panic, abort, or loop forever on any input, and SHALL NOT allocate memory based on a length the peer claims. Memory held for one stream while decoding SHALL be bounded by the 6-byte header plus the 65,536-byte frame limit.

#### Scenario: Arbitrary bytes on a stream
- **WHEN** a receiver is fed any sequence of bytes as stream input
- **THEN** each step yields a frame, a request for more input, or a protocol error, and never panics

#### Scenario: Header claims a huge payload
- **WHEN** a header's payload length is 4,294,967,295
- **THEN** the receiver reports a protocol error without allocating memory for the claimed length

### Requirement: Pairing and membership message layouts
The pairing and membership messages SHALL have these payloads, all integers little-endian:
- `PairPake`: exactly 33 bytes, a SPAKE2 symmetric-mode message.
- `PairConfirm`: a 32-byte key-confirmation tag, a name length `n` (u8, 1 to 63), and `n` bytes of UTF-8 machine name.
- `MemberRecord`: a kind (u8: `1` addition, `2` removal), the subject's 32-byte peer identity, the epoch (u64), the signer's 32-byte peer identity, a name length `n` (u8; 1 to 63 for an addition, 0 for a removal), `n` bytes of UTF-8 machine name, and a 64-byte Ed25519 signature.
- `RecordsDone`: empty.

Any other kind, a name length outside its range, invalid UTF-8, or a payload whose length does not match its fields SHALL be a protocol error.

#### Scenario: Removal record layout
- **WHEN** a removal record is encoded
- **THEN** its payload is 1 + 32 + 8 + 32 + 1 + 64 = 138 bytes, with name length `0`

#### Scenario: Unknown record kind
- **WHEN** a `MemberRecord` payload has kind `3`
- **THEN** the receiver reports a protocol error

#### Scenario: Addition with no name
- **WHEN** a `MemberRecord` payload has kind `1` and name length `0`
- **THEN** the receiver reports a protocol error

#### Scenario: RecordsDone with a payload
- **WHEN** a `RecordsDone` frame carries 1 byte of payload
- **THEN** the receiver reports a protocol error
