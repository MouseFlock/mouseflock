# Spec Delta

## Purpose

Lets text copied on one DeskHop machine be pasted on another. Copied text follows the cursor: a copy is announced to the desk, and its text moves only to the machines the cursor crosses into.

## ADDED Requirements

### Requirement: Copies are announced without their text
When the session agent reports a shareable text copy, the service SHALL give it a new random 64-bit ID, keep its text in memory as this machine's held copy, and send `ClipboardOffer` with that ID to every up member. The offer SHALL NOT carry any of the text.

#### Scenario: Copy on A
- **WHEN** A's agent reports that `hello` was copied, and B and C are up members
- **THEN** B and C each receive one `ClipboardOffer` from A, and neither receives the text

### Requirement: The newest copy wins
Each machine SHALL treat the copy it learned of most recently as the newest. That is either its own reported copy or a member's `ClipboardOffer`, in the order the service received them. A later copy SHALL replace an earlier one, whichever machine made either.

#### Scenario: Copies on two machines
- **WHEN** C hears A's offer and then B's offer
- **THEN** C's newest copy is B's

#### Scenario: Own copy after an offer
- **WHEN** C hears A's offer, and then C's own agent reports a copy
- **THEN** C's newest copy is its own, and C fetches nothing from A

### Requirement: Text follows the cursor
When the cursor arrives at this machine, the service SHALL fetch the newest copy if a member made it, this machine does not already hold it, and that member is up. The cursor arrives when focus becomes controlled by a member, or returns here from forwarding. The service SHALL then send `ClipboardRequest` with that copy's ID, and give the received text to the agent to set.

#### Scenario: Copy on A, cross to B
- **WHEN** A's user copies `hello` and the cursor then crosses from A into B
- **THEN** B requests the copy from A, and B's agent is told to set `hello`

#### Scenario: Copy on B, back to A
- **WHEN** the cursor is on B, `world` is copied there, and the cursor crosses back to A
- **THEN** A requests the copy from B, and A's agent is told to set `world`

#### Scenario: Nothing newer
- **WHEN** the cursor crosses into B and B already holds the newest copy
- **THEN** B sends no request

#### Scenario: Owner is down
- **WHEN** the newest copy is A's, A is down, and the cursor arrives at B
- **THEN** B sends no request and keeps its clipboard

### Requirement: Text is sent only on request
The service SHALL send clipboard text only to an up member that sent `ClipboardRequest` for the ID of this machine's held copy. A request for any other ID SHALL be answered with nothing.

#### Scenario: Stale request
- **WHEN** B requests A's earlier copy after A's user copied something newer
- **THEN** A sends B no text

### Requirement: Private and unsupported copies are not shared
A copy the agent reports as not shared SHALL NOT be announced or sent. That covers private copies (marked to be excluded from clipboard monitoring, history or cloud sync), copies with no text, and text over 4 MiB. Such a copy SHALL clear this machine's held copy, so the earlier copy is no longer sent to anyone.

#### Scenario: Password copied
- **WHEN** a password manager on A copies a password marked as excluded from clipboard history
- **THEN** no member receives an offer or text for it

#### Scenario: Private copy after a shareable one
- **WHEN** A's user copies `hello`, then copies a private password, and B then requests `hello`'s ID
- **THEN** A sends B no text

### Requirement: Received text is not announced again
Text that this machine's agent sets because a member sent it SHALL NOT be offered again by this machine. This machine SHALL record that it now holds that copy, under the ID its owner gave it.

#### Scenario: No echo
- **WHEN** B receives A's copy and its agent sets it
- **THEN** B sends no `ClipboardOffer`, and when the cursor later returns to A, A fetches nothing

### Requirement: Transfers never delay input
Clipboard text SHALL travel only on a unidirectional QUIC stream of its own, opened by the sender for one transfer. It SHALL never travel on the control stream. A connection SHALL accept at most one incoming clipboard stream at a time. A machine SHALL wait at most 10 seconds for a requested copy, and SHALL discard text that arrives after a newer copy was learned.

#### Scenario: Typing during a large transfer
- **WHEN** a 4 MiB copy is being transferred from A to B while A forwards keystrokes to B
- **THEN** the keystrokes arrive without waiting for the transfer to finish

#### Scenario: Newer copy during a transfer
- **WHEN** B is receiving A's copy and B's own user copies something before it finishes
- **THEN** B does not set A's text when it arrives

### Requirement: Clipboard stream layout and limits
A clipboard stream SHALL carry one `ClipboardData` frame (the copy's ID and the text's length in bytes), then `ClipboardChunk` frames whose bytes total exactly that length, then end. The length SHALL be at most 4,194,304 bytes, and the text valid UTF-8. A breach SHALL be a protocol error. Text a machine did not request SHALL be discarded unset.

#### Scenario: Too long
- **WHEN** a clipboard stream's `ClipboardData` claims 5,000,000 bytes
- **THEN** the receiver closes the connection with a protocol error, before reading the text

#### Scenario: Unrequested text
- **WHEN** B receives a well-formed clipboard stream for an ID it did not request
- **THEN** B discards it and its agent is told nothing

### Requirement: Clipboard messages travel between up members
`ClipboardOffer` and `ClipboardRequest` SHALL be sent only on the control stream of a member connection whose peer is up, and clipboard streams only on such a connection. Either message before the peer is up, any of them on a pairing connection, or on the wrong kind of stream SHALL be a protocol error. A clipboard stream that arrives before the peer's records SHALL be held unread until they arrive, since QUIC does not order streams.

#### Scenario: Offer on a pairing connection
- **WHEN** a pairing connection receives `ClipboardOffer`
- **THEN** the receiver closes it with a protocol error

#### Scenario: Data on the control stream
- **WHEN** a member's control stream carries `ClipboardChunk`
- **THEN** the receiver closes the connection with a protocol error

### Requirement: Clipboard text stays out of logs and off disk
The service SHALL NOT write clipboard text to its log or to any file. It SHALL hold at most this machine's held copy and one incoming transfer per connection in memory. Logs MAY record that a copy was offered, fetched or set, and its length, but never its text.

#### Scenario: Log after a transfer
- **WHEN** `hello` is copied on A and fetched by B
- **THEN** neither service log contains `hello`
