## MODIFIED Requirements

### Requirement: Advertisement
A running machine SHALL advertise one mDNS service of type `_mouseflock._udp.local.`. The instance name SHALL be the first 16 bytes of its peer identity in lowercase hexadecimal, and the port SHALL be the one it listens on. The TXT record SHALL hold `id=` followed by the full 32-byte peer identity in lowercase hexadecimal, and SHALL hold `pair=1` exactly while pairing mode is open. The machine SHALL update the advertisement when pairing mode opens or closes.

#### Scenario: Advertised identity
- **WHEN** a machine with peer identity `ab12…` (32 bytes) is running
- **THEN** it advertises instance `ab12…` (32 hex digits) with TXT `id=` and the 64 hex digits of its identity, and no `pair` entry

#### Scenario: Pairing mode open
- **WHEN** the user opens pairing mode
- **THEN** the advertisement's TXT gains `pair=1`, and loses it when pairing mode closes

### Requirement: Finding members
A machine SHALL browse for `_mouseflock._udp.local.` and treat each resolved advertisement as a sighting of the identity in its `id=` entry, at the advertised addresses and port. Sightings with a missing or malformed `id=`, and sightings of its own identity, SHALL be ignored. A sighting of a member that is not up SHALL lead to a dial, as the peer-transport capability specifies. A sighting SHALL never be trusted on its own: the dial pins the identity.

#### Scenario: Member found
- **WHEN** member B's advertisement appears
- **THEN** A dials B at the advertised address and port

#### Scenario: Spoofed advertisement
- **WHEN** machine X advertises B's identity at X's own address
- **THEN** A's dial to that address fails pinning, and nothing else changes

#### Scenario: Malformed identity
- **WHEN** an advertisement's TXT has `id=` followed by 63 hex digits
- **THEN** it is ignored
