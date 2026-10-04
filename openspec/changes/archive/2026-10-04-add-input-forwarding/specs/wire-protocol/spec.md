# Spec Delta

## MODIFIED Requirements

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
