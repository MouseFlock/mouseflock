# pairing Specification

## Purpose

Lets a user add a machine to their desk by typing a short digit code shown on another machine, and gives every machine a long-lived identity that peers pin. Pairing is authenticated by a PAKE, so the code never crosses the network and an attacker gets one online guess per attempt.

## Requirements

### Requirement: Machine identity
Each machine SHALL have one Ed25519 identity key, generated from the operating system's random number generator the first time it is needed and kept from then on. The machine's peer identity SHALL be the 32-byte Ed25519 public key. The identity SHALL be stored as a 37-byte identity file: the ASCII bytes `MFID`, a format version byte `1`, and the 32-byte private key seed. Reading an identity file that is not exactly this layout SHALL report an error and SHALL NOT generate a replacement key, because a new key would silently drop the machine from its desk.

#### Scenario: First start creates an identity
- **WHEN** no identity file exists
- **THEN** a new key is generated and its identity file is produced, and the peer identity is the key's public half

#### Scenario: Identity survives a restart
- **WHEN** an identity file is written and read back
- **THEN** the same peer identity results, and signatures made before and after verify under it

#### Scenario: Corrupt identity file
- **WHEN** an identity file has the wrong length, magic, or version
- **THEN** reading it reports an error and no new key is generated

### Requirement: Pairing code format
A pairing code SHALL be written `<locator>-<digits>`. The locator SHALL be the host part of the inviting machine's IPv4 address in whole bytes: `4 - min(⌊prefix length / 8⌋, 3)` bytes, written in decimal and separated by `.`, so 1 byte on a /24, 2 on a /16, 3 on a /8. The digits SHALL be a 6-digit secret, drawn uniformly from the operating system's random number generator, followed by one check digit computed with the Damm algorithm over the locator's decimal digits and then the secret's. When parsing typed input, whitespace anywhere and hyphens after the first SHALL be ignored. Input that has no locator, a locator byte above 255, more locator bytes than 4, other than 7 digits after the locator, any other character, or a wrong check digit SHALL be rejected before any connection is made.

#### Scenario: Code on a /24 network
- **WHEN** a machine at `192.168.1.137/24` creates a code with secret `482915`
- **THEN** the code's locator is `137` and its text is `137-482915` followed by the Damm check digit of `137482915`

#### Scenario: Code on a /16 network
- **WHEN** a machine at `10.20.1.137/16` creates a code
- **THEN** the code's locator is `1.137`

#### Scenario: Typed with spaces and extra hyphens
- **WHEN** the user types `137 - 482 915-<check>` for a valid code `137-482915<check>`
- **THEN** it parses to the same code

#### Scenario: One mistyped digit
- **WHEN** the user types a valid code with one digit changed, or with two adjacent digits swapped
- **THEN** parsing rejects it as a typo and no pairing attempt is made

#### Scenario: Locator out of range
- **WHEN** the user types a code whose locator contains `300`
- **THEN** parsing rejects it

### Requirement: Inviter address from the code
The joining machine SHALL form the inviting machine's IPv4 address by taking the leading bytes of its own IPv4 address and replacing the trailing bytes with the locator. A joining machine without an IPv4 address SHALL NOT form an address from the code.

#### Scenario: Same /24 subnet
- **WHEN** a machine at `192.168.1.52` parses the code with locator `137`
- **THEN** the inviter's address is `192.168.1.137`

#### Scenario: Two-byte locator
- **WHEN** a machine at `10.20.7.9` parses the code with locator `1.137`
- **THEN** the inviter's address is `10.20.1.137`

### Requirement: Pairing exchange
Pairing SHALL run on a connection between the joiner (the machine where the code was typed) and the inviter (the machine showing it), after the version handshake, in this order:
1. The joiner sends `PairPake`. The inviter replies with its own `PairPake`. Both messages are SPAKE2 messages in symmetric mode whose password is the code's canonical text.
2. The joiner sends `PairConfirm` with its key-confirmation tag and its machine name.
3. The inviter checks the tag. If it is wrong, the inviter closes with close reason `3` (wrong pairing code) and does not send its own tag. If it is right, the inviter sends `PairConfirm` with its tag and name.
4. The joiner checks the inviter's tag. If it is wrong, the joiner closes with close reason `3`.
5. Both sides exchange membership records as defined by the desk-membership capability. The joiner then closes the connection normally. Pairing is complete on each side once its records are exchanged.

Any pairing message out of this order SHALL be a protocol error.

#### Scenario: Matching codes
- **WHEN** the joiner uses the code the inviter is showing
- **THEN** both sides accept each other's tags, each learns the other's peer identity and machine name, and pairing completes on both

#### Scenario: Wrong code
- **WHEN** the joiner uses a code that is well-formed but not the inviter's
- **THEN** the inviter closes with close reason `3` without sending its tag, and the joiner reports a wrong code

#### Scenario: Message out of order
- **WHEN** the joiner sends `PairConfirm` before `PairPake`
- **THEN** the inviter reports a protocol error

### Requirement: Key confirmation bound to the connection
Each side's key-confirmation tag SHALL be derived from the SPAKE2 shared key together with the connection's TLS exporter value and the public keys in both sides' TLS certificates, ordered inviter then joiner, with different derivations for the joiner's tag and the inviter's tag. A side SHALL compare a received tag in constant time. Tags SHALL NOT reveal the code: an observer of the whole exchange SHALL learn nothing that lets it test candidate codes offline.

#### Scenario: Relay in the middle
- **WHEN** a third machine relays the pairing between joiner and inviter, so that the two sides see different TLS exporter values or certificate keys
- **THEN** the inviter rejects the joiner's tag even though the code is correct, and closes with close reason `3`

#### Scenario: Reflected tag
- **WHEN** a side receives a tag equal to the one it sent
- **THEN** it rejects it

### Requirement: Machine names
A machine name in `PairConfirm` SHALL be 1 to 63 bytes of UTF-8. A name outside these bounds, or invalid UTF-8, SHALL be a protocol error.

#### Scenario: Empty name
- **WHEN** a `PairConfirm` carries a name of 0 bytes
- **THEN** the receiver reports a protocol error

### Requirement: Pairing mode and limits
The inviter SHALL accept pairing only while pairing mode is open. Pairing mode SHALL open when the user asks for it, creating a new code, and SHALL close after 10 minutes, after one successful pairing, after 3 failed attempts, or when the user closes it. The code SHALL be valid only while the pairing mode that created it is open, so each opening allows at most 3 guesses. The inviter SHALL handle one attempt at a time. An attempt begins when the inviter receives the joiner's `PairPake`. An attempt that does not complete within 30 seconds, ends with a wrong tag, ends with a protocol error, or ends because the connection drops before completion SHALL count as a failed attempt. When pairing mode is closed, or another attempt is in progress, the inviter SHALL answer a `PairPake` by closing with close reason `4` (not ready to pair), without counting an attempt.

#### Scenario: Pairing mode closed
- **WHEN** a joiner sends `PairPake` to a machine not in pairing mode
- **THEN** that machine closes with close reason `4` and the joiner reports that the machine is not ready to pair

#### Scenario: Window expires
- **WHEN** 10 minutes pass after the user opened pairing mode without a successful pairing
- **THEN** pairing mode is closed and the code is no longer accepted

#### Scenario: Third wrong guess closes pairing mode
- **WHEN** three attempts fail while pairing mode is open
- **THEN** pairing mode closes, the inviter tells the user there were too many wrong codes, and the shown code fails even if typed correctly

#### Scenario: Two wrong guesses
- **WHEN** two attempts fail and the third uses the shown code
- **THEN** pairing completes

#### Scenario: Abandoned attempt
- **WHEN** a joiner sends `PairPake` and then sends nothing for 30 seconds
- **THEN** the attempt counts as failed and the inviter accepts a new attempt

#### Scenario: Concurrent attempt
- **WHEN** a second joiner sends `PairPake` while an attempt is in progress
- **THEN** the inviter closes the second connection with close reason `4` and the first attempt continues

#### Scenario: One join per opening
- **WHEN** a pairing completes
- **THEN** pairing mode closes, and the next machine needs the user to open pairing mode again
