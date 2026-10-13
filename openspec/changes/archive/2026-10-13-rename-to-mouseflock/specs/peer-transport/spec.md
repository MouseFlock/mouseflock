## MODIFIED Requirements

### Requirement: Two kinds of connection
A connection SHALL use ALPN `mouseflock/1` for a member connection or `mouseflock-pair/1` for a pairing connection, and any other ALPN SHALL fail the handshake. The listener SHALL accept any client key during the TLS handshake and decide afterwards, as these requirements describe. The dialer SHALL open the first bidirectional stream as the control stream, and both sides SHALL send `Hello` on it first, as the wire protocol specifies. The pairing and membership messages SHALL travel on the control stream.

#### Scenario: Unknown ALPN
- **WHEN** a client offers only ALPN `h3`
- **THEN** the handshake fails
