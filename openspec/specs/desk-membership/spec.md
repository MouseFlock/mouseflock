# desk-membership Specification

## Purpose

Keeps every machine in a desk trusting every other one, after a new machine has paired with just one member. Members sign records that add or remove a machine, accept records signed by any member, and exchange them on every connection, so removals stick and members that were offline catch up.

## Requirements

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

### Requirement: Membership view
A machine's membership view SHALL hold, for each subject, the one record that wins: the record with the highest epoch, and on equal epochs the removal. A subject SHALL be a member when its winning record is an addition. A machine SHALL always count itself as a member of its own view. A machine SHALL keep only the winning record for each subject, and SHALL hold records for at most 256 subjects.

#### Scenario: Later removal wins
- **WHEN** a machine holds an addition of C at epoch 1 and accepts a removal of C at epoch 2
- **THEN** C is not a member

#### Scenario: Stale addition loses
- **WHEN** a machine holds a removal of C at epoch 2 and receives an addition of C at epoch 1
- **THEN** C stays removed

#### Scenario: Removal wins a tie
- **WHEN** a machine receives an addition and a removal of C, both at epoch 3
- **THEN** C is not a member

#### Scenario: Re-pairing a removed machine
- **WHEN** a removed machine pairs again with a member
- **THEN** the member signs an addition at an epoch above the removal, and the machine is a member again

### Requirement: Epochs follow the wall clock
A machine signing a record SHALL give it the epoch that is the greater of the current Unix time in seconds and one above the highest epoch it holds for that subject, and at least 1. Later actions therefore win on every member, whichever member signed them and whatever it had received before.

#### Scenario: Epoch is the current time
- **WHEN** a machine holding no record for C adds C at Unix time 1,800,000,000
- **THEN** the record's epoch is 1,800,000,000

#### Scenario: Clock behind a held record
- **WHEN** a machine holds a record for C at epoch 2,000,000,000 and removes C at Unix time 1,800,000,000
- **THEN** the removal's epoch is 2,000,000,001

#### Scenario: Re-pairing through a member that missed the removal
- **WHEN** B removed C, and later C pairs with A, which never received the removal
- **THEN** after A and B exchange records, C is a member in both views

### Requirement: Accepting records
A machine SHALL accept a record only if its signature is valid and its signer is a member of the receiving machine's view, the receiving machine itself included. A machine SHALL keep a record whose signer later stops being a member. Records received in one exchange whose signer is not yet a member SHALL be checked again after the other records of that exchange are accepted, until no more can be accepted; the rest SHALL be dropped.

#### Scenario: Record from a stranger
- **WHEN** a machine receives a validly signed record whose signer is not a member
- **THEN** it drops the record

#### Scenario: Records out of dependency order
- **WHEN** one exchange delivers an addition of E signed by D before the addition of D signed by a member
- **THEN** both are accepted at the end of the exchange

### Requirement: Joining by pairing
When pairing completes, each side SHALL sign an addition of the other side, with its epoch chosen as in "Epochs follow the wall clock", add it to its own view, and then send every winning record it holds, the new one included, followed by `RecordsDone`. If both sides were already in desks, the desks SHALL merge into one. Neither side SHALL have a role over the other.

#### Scenario: Third machine joins through one member
- **WHEN** A and B are in a desk, and C pairs with B
- **THEN** C's view contains A and B, and B's view contains C; once A and B exchange records, A's view contains C, though A never paired with C

#### Scenario: Two desks merge
- **WHEN** A is in a desk with C, B is in a desk with D, and A pairs with B
- **THEN** after the records spread, each of A, B, C and D has the other three as members

### Requirement: Removing a machine
A member SHALL be able to remove any machine, itself included, by signing a removal, with its epoch chosen as in "Epochs follow the wall clock". A machine that accepts a winning removal of itself SHALL forget the desk: it drops every record and is left with only itself. Its identity key SHALL be kept.

#### Scenario: Remove from any member
- **WHEN** B removes C, and later A and B exchange records
- **THEN** C is not a member in A's or B's view

#### Scenario: Removed machine forgets the desk
- **WHEN** C receives a removal of itself from a member
- **THEN** C's view contains only C, and C keeps its peer identity

### Requirement: Record exchange between members
On every connection between two members, after the version handshake, each side SHALL send every winning record it holds, followed by `RecordsDone`. A receiver SHALL apply the exchange's records when `RecordsDone` arrives. A `MemberRecord` after `RecordsDone` SHALL be accepted as a single update at once, with no further exchange marker. More than 256 records before `RecordsDone` SHALL be a protocol error.

#### Scenario: Offline member catches up
- **WHEN** A was offline while C joined through B, and A then connects to B
- **THEN** B's records include the addition of C, and A's view contains C after B's `RecordsDone`

#### Scenario: Record flood
- **WHEN** a peer sends 257 `MemberRecord` messages before `RecordsDone`
- **THEN** the receiver reports a protocol error

### Requirement: Trust store file
A machine's membership view SHALL be stored as a trust store file: the ASCII bytes `MFTS`, a format version byte `1`, a record count (u16, little-endian), then each record as a length (u16, little-endian) followed by its wire encoding. Reading a trust store file that does not match this layout, or contains a record whose signature does not verify, SHALL report an error.

#### Scenario: Round trip
- **WHEN** a view is written to a trust store file and read back
- **THEN** the same members, records, and epochs result

#### Scenario: Corrupt trust store
- **WHEN** a trust store file is truncated, or one of its records has a bad signature
- **THEN** reading it reports an error
