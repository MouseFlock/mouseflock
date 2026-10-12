# peer-transport Specification

## Purpose

Connects MouseFlock machines over QUIC so that only members of the same desk can talk to each other, a new machine can pair, and each machine knows promptly which peers are up. It carries the pairing and membership exchanges today and the input messages later.

## Requirements

### Requirement: Fixed port
A machine SHALL listen for QUIC on UDP port 47391 on all its IPv4 interfaces, and SHALL dial peers on that port unless an mDNS advertisement names another. If the port cannot be bound, starting SHALL fail with an error that names the port.

#### Scenario: Port already in use
- **WHEN** another program holds UDP 47391 and the machine starts its transport
- **THEN** starting fails with an error naming port 47391

### Requirement: Identity certificates
Each machine SHALL present, on every connection and in both directions, a self-signed X.509 certificate whose public key is its Ed25519 identity key, generated anew at every start. A peer's identity SHALL be read only from its certificate's public key; names, validity dates, and other certificate fields SHALL be ignored. The TLS handshake SHALL prove that each side holds the private key of the certificate it presents.

#### Scenario: Identity from the certificate
- **WHEN** a machine connects to another
- **THEN** each learns the other's peer identity as the 32-byte Ed25519 key in the presented certificate

#### Scenario: Certificate without the private key
- **WHEN** a peer presents another machine's certificate but cannot sign with its key
- **THEN** the TLS handshake fails and no connection is made

### Requirement: Dialing pins the expected key
A machine dialing a member SHALL accept the connection only if the listener's certificate key equals that member's peer identity. A machine dialing to pair SHALL accept any listener key, and SHALL use it as the inviter's identity in pairing.

#### Scenario: Wrong machine at a member's address
- **WHEN** A dials member B at an address where machine X now answers
- **THEN** the handshake fails, no connection is made, and nothing is sent to X

### Requirement: Two kinds of connection
A connection SHALL use ALPN `mouseflock/1` for a member connection or `mouseflock-pair/1` for a pairing connection, and any other ALPN SHALL fail the handshake. The listener SHALL accept any client key during the TLS handshake and decide afterwards, as these requirements describe. The dialer SHALL open the first bidirectional stream as the control stream, and both sides SHALL send `Hello` on it first, as the wire protocol specifies. The pairing and membership messages SHALL travel on the control stream.

#### Scenario: Unknown ALPN
- **WHEN** a client offers only ALPN `h3`
- **THEN** the handshake fails

### Requirement: Member connections
On a member connection, after the version handshake, both sides SHALL send every winning membership record they hold followed by `RecordsDone`, as the desk-membership capability specifies. The dialer, which has pinned the listener's key, SHALL send first. The listener SHALL apply the dialer's records before deciding whether the dialer is a member, so a machine that joined through another member can introduce itself. If, after the dialer's `RecordsDone`, the dialer's key is not a member of the listener's view, the listener SHALL close with close reason `5` (not a member) without sending any record. Otherwise the listener SHALL then send its own records. A peer SHALL count as up on a machine once both `RecordsDone` messages have been exchanged and each side is a member of the other's view.

#### Scenario: Members connect
- **WHEN** A and B are members of each other's desks and A dials B
- **THEN** after the record exchange both report the other as up

#### Scenario: Introduced by records
- **WHEN** C joined through B, A has not yet received the record adding C, and C dials A
- **THEN** A accepts the record adding C from C's exchange, because B signed it, and both report the other as up

#### Scenario: Stranger on a member connection
- **WHEN** a machine that is not in A's desk dials A as a member and its records do not make it one
- **THEN** A closes the connection with close reason `5`, having sent no records, and reports nothing up

### Requirement: Pairing connections
A pairing connection SHALL run the pairing exchange from the pairing capability: the dialer is the joiner and the listener is the inviter. The inviter SHALL handle any `PairPake` it receives on a pairing connection through its pairing mode, whether or not the joiner's key is already a member. When pairing completes, each side SHALL treat the other as a member and dial it over a member connection.

#### Scenario: Pairing then connecting
- **WHEN** B pairs with A by code
- **THEN** the pairing connection closes normally, a member connection follows, and both report the other as up

#### Scenario: Pairing mode closed
- **WHEN** a joiner sends `PairPake` to a machine whose pairing mode is closed
- **THEN** that machine closes the pairing connection with close reason `4`

### Requirement: One connection per pair
Two machines SHALL keep at most one member connection between them. When both have dialed and two connections are up, both SHALL keep the one dialed by the machine with the lower peer identity, comparing the 32 bytes in order, and close the other with close reason `0`, without reporting the peer down.

#### Scenario: Both dial at once
- **WHEN** A and B dial each other at the same time and both connections complete
- **THEN** each keeps the connection dialed by the lower identity, closes the other, and reports the peer up once and never down

### Requirement: Liveness
A machine SHALL send a keepalive on every member connection when it has sent nothing else for 250 ms. A machine SHALL report a peer down as lost when it has received nothing from the peer for 1 second. On request, a machine SHALL close every member connection with close reason `0` (goodbye) and report each peer down as having said goodbye; until told to resume, it SHALL neither dial members nor accept member connections, closing them with close reason `0`. A machine receiving close reason `0` outside the one-connection rule SHALL report the peer down at once as having said goodbye.

#### Scenario: Peer goes silent
- **WHEN** B stops answering, for example because its network cable is pulled
- **THEN** A reports B down as lost between 1 and 2 seconds after the last packet from B

#### Scenario: Goodbye before sleep
- **WHEN** B says goodbye
- **THEN** A reports B down as having said goodbye without waiting for the timeout

#### Scenario: Quiet until resumed
- **WHEN** B has said goodbye
- **THEN** B does not reconnect to A until it is told to resume, and then reconnects

### Requirement: Reconnecting
A machine SHALL dial every member that is not up, at the addresses of its mDNS advertisements and at its last-known addresses. After a failed dial or a loss, it SHALL wait before dialing that member again, starting at 0.5 seconds and doubling up to 30 seconds. A new advertisement from that member SHALL end the wait.

#### Scenario: Peer comes back
- **WHEN** B was lost and then advertises again
- **THEN** A dials B at once, and both report the other up

#### Scenario: Backoff
- **WHEN** dials to B keep failing
- **THEN** A waits 0.5, 1, 2, 4, 8, 16, then 30 seconds between attempts, and 30 seconds from then on

### Requirement: Membership changes reach members
When a machine's view gains or changes a record, it SHALL send that record at once to every member that is up. When a member is removed from the view, the machine SHALL close its connection to it with close reason `5` and report it down as removed, after sending it the removal. A machine that accepts a winning removal of itself SHALL close every connection with close reason `0` and report every peer down as removed.

#### Scenario: Removal reaches the removed machine
- **WHEN** A removes B while both are up
- **THEN** B receives the removal, forgets the desk, and both report the other down as removed

#### Scenario: New member spreads
- **WHEN** C pairs with B while A and B are up
- **THEN** B sends A the record adding C at once, and A dials C

### Requirement: Limits on strangers
A connection whose key is not yet a member SHALL send `PairPake` (pairing connections) or finish introducing itself (member connections) within 10 seconds of the handshake, or be closed: pairing connections with close reason `4`, member connections with close reason `5`. A machine SHALL hold at most 8 such connections at once, and SHALL refuse more with the same close reasons until one ends.

#### Scenario: Idle stranger
- **WHEN** a stranger completes the handshake on a pairing connection and sends nothing for 10 seconds
- **THEN** the connection is closed with close reason `4`

#### Scenario: Ninth stranger
- **WHEN** 8 stranger connections are open and a ninth completes its handshake
- **THEN** the ninth is closed at once and the first 8 are unaffected

### Requirement: State for the service to store
A machine SHALL report its trust store file contents whenever its membership view changes, and SHALL report the addresses at which each member was last reached whenever they change. A machine SHALL accept both at start-up and use the addresses for reconnecting.

#### Scenario: Restart on a network without multicast
- **WHEN** A and B paired on a network that blocks multicast and both restart with their stored trust store and addresses
- **THEN** they reconnect without any mDNS advertisement
