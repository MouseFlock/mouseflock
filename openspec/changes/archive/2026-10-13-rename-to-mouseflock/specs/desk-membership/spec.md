## MODIFIED Requirements

### Requirement: Membership records
A membership record SHALL state that one machine (the subject) is added to or removed from the desk. It SHALL carry the subject's peer identity, an epoch (an unsigned 64-bit number that orders records about the same subject), the signer's peer identity, for an addition the subject's machine name (1 to 63 bytes of UTF-8), and the signer's Ed25519 signature. The signature SHALL cover the ASCII bytes `mouseflock member record v1` followed by the record's wire encoding without the signature. A removal SHALL carry no name. An addition whose signer is its subject SHALL be invalid.

#### Scenario: Valid signature
- **WHEN** a member signs a record and another machine checks it
- **THEN** the signature verifies under the signer's peer identity

#### Scenario: Tampered record
- **WHEN** any byte of a signed record other than the signature is changed
- **THEN** the signature no longer verifies and the record is rejected

#### Scenario: Self-signed addition
- **WHEN** a record adds the machine that signed it
- **THEN** it is rejected

### Requirement: Trust store file
A machine's membership view SHALL be stored as a trust store file: the ASCII bytes `MFTS`, a format version byte `1`, a record count (u16, little-endian), then each record as a length (u16, little-endian) followed by its wire encoding. Reading a trust store file that does not match this layout, or contains a record whose signature does not verify, SHALL report an error.

#### Scenario: Round trip
- **WHEN** a view is written to a trust store file and read back
- **THEN** the same members, records, and epochs result

#### Scenario: Corrupt trust store
- **WHEN** a trust store file is truncated, or one of its records has a bad signature
- **THEN** reading it reports an error
