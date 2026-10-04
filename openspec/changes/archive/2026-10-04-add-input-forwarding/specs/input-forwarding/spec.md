# Spec Delta

## Purpose

Carries what one machine's engine tells another's (focus changes, layout corrections, and forwarded keys, buttons, wheel and motion) between members of a desk, so that input forwarded across the LAN arrives complete and in order, and motion stays smooth and correct when datagrams are lost.

## ADDED Requirements

### Requirement: Engine messages travel between up members
Engine messages SHALL be sent only on a member connection whose peer is up, on its control stream, in the order the engine produced them. An engine message received on a pairing connection, or on a member connection before the peer is up, SHALL be a protocol error. A `Motion` datagram received before the peer is up SHALL be dropped.

#### Scenario: Message between up members
- **WHEN** A and B are up and A's engine sends `FocusEnter` to B
- **THEN** B reports that `FocusEnter` from A

#### Scenario: Engine message before the records
- **WHEN** a member connection has agreed the version but not finished the record exchange, and the dialer sends `FocusEnter`
- **THEN** the listener closes the connection with close reason `1` (protocol error)

#### Scenario: Engine message on a pairing connection
- **WHEN** a joiner sends a key message on a pairing connection
- **THEN** the inviter closes the connection with close reason `1`

### Requirement: Messages keep their order
Engine messages and forwarded keys, buttons and wheel from one machine to another SHALL be reported by the receiver in the order they were sent, with none lost while the connection stays up.

#### Scenario: Typing a word
- **WHEN** A forwards the presses and releases for "hello" to B
- **THEN** B reports the ten key messages in the same order

### Requirement: Motion as running totals
Forwarded motion SHALL be sent in `Motion` datagrams carrying a sequence number and the running totals of horizontal and vertical movement sent on that connection since the peer came up. The receiver SHALL report as movement the difference between the newest totals and the totals it last applied. A datagram whose sequence number is not newer than the last one applied SHALL be ignored. Totals and sequence numbers SHALL wrap around without error.

#### Scenario: A lost datagram
- **WHEN** A sends three `Motion` datagrams for movements of 5, 5 and 5 to the right, and the second is lost
- **THEN** B reports movements totalling 15 to the right

#### Scenario: A late datagram
- **WHEN** B has applied `Motion` sequence 8 and then receives sequence 7
- **THEN** B ignores sequence 7

#### Scenario: Totals wrap around
- **WHEN** the horizontal total passes the largest signed 32-bit value
- **THEN** the receiver still reports the movement that was sent

### Requirement: Motion coalescing
A connection SHALL send at most one `Motion` datagram every 4 ms. Movement that arrives sooner SHALL be added to the next datagram, which SHALL be sent no later than 4 ms after the previous one. When no movement has arrived for 20 ms after a `Motion` datagram, that datagram's sequence number and totals SHALL be sent once more.

#### Scenario: A high-rate mouse
- **WHEN** the engine forwards 8 movements of 1 to the right within 1 ms
- **THEN** at most two `Motion` datagrams are sent for them, and their totals end 8 to the right

#### Scenario: The last datagram is repeated
- **WHEN** movement stops after a `Motion` datagram with sequence 40
- **THEN** sequence 40 with the same totals is sent again 20 ms later, and nothing more until the next movement

### Requirement: Keys, buttons and wheel carry the motion mark
Every key, button and wheel message SHALL carry a sequence number and the totals of all movement forwarded before it, including movement still waiting to be coalesced; that movement SHALL then need no `Motion` datagram of its own. Before reporting a key, button or wheel message, the receiver SHALL report as movement any part of those totals it has not yet applied.

#### Scenario: Click after a lost datagram
- **WHEN** A forwards a movement of 30 to the right, the `Motion` datagram carrying it is lost, and A then forwards a left-button press
- **THEN** B reports a movement of 30 to the right and then the button press

#### Scenario: Datagram arrives after the click
- **WHEN** B has applied the motion mark carried by a button press, and the `Motion` datagram with that same sequence number then arrives
- **THEN** B reports no further movement

### Requirement: Malformed engine messages
An engine message or `Motion` datagram whose payload does not match its layout, or whose side, button, or pressed-or-released field holds a value the layout does not define, SHALL be a protocol error.

#### Scenario: Unknown button
- **WHEN** a button message names button 9
- **THEN** the receiver closes the connection with close reason `1`
