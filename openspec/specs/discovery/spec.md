# discovery Specification

## Purpose

Lets MouseFlock machines find each other on the LAN with no addresses typed or configured: members are found by mDNS, and a joining machine finds the inviter from the pairing code, with or without multicast.

## Requirements

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

### Requirement: Inviter address for the code
When pairing mode opens, the inviter SHALL build its code from the IPv4 address and prefix length of the interface that carries its default route: the source address the operating system picks for a public destination, found without sending any packet. Without a default route, it SHALL use the first up, non-loopback IPv4 interface with a private address, then any up, non-loopback IPv4 interface. Without any, opening pairing mode SHALL fail with an error that says no IPv4 network was found.

#### Scenario: Laptop with Wi-Fi and a VPN
- **WHEN** a machine has Wi-Fi `192.168.1.137/24` carrying the default route and a VPN adapter `10.8.0.5/24`
- **THEN** its code's locator is `137`

#### Scenario: No IPv4 network
- **WHEN** a machine has no up, non-loopback IPv4 interface and the user opens pairing mode
- **THEN** opening fails, saying no IPv4 network was found

### Requirement: Joiner candidates
A joiner SHALL dial these candidates at once, on the advertised port where there is one and on 47391 otherwise: the address rebuilt from the code for each of its own up, non-loopback IPv4 interfaces, and the IPv4 addresses of `pair=1` advertisements whose trailing bytes equal the code's locator. Duplicate addresses SHALL be dialed once. The first candidate that answers the joiner's `PairPake` with its own SHALL carry on the pairing; the joiner SHALL close the others with close reason `0`. A candidate that does not complete its handshake within 5 seconds SHALL count as unreachable. When every candidate has failed, the joiner SHALL report a wrong code if any candidate closed with close reason `3`; otherwise that the machine was not ready to pair if any closed with close reason `4`; otherwise that the machine could not be reached.

#### Scenario: Same subnet
- **WHEN** a joiner at `192.168.1.52/24` uses a code with locator `137`
- **THEN** it dials `192.168.1.137` on port 47391

#### Scenario: Multicast blocked
- **WHEN** the network drops mDNS and the joiner is on the inviter's subnet
- **THEN** pairing succeeds through the address rebuilt from the code

#### Scenario: Different subnet, found by mDNS
- **WHEN** the joiner is on another subnet that the inviter's `pair=1` advertisement reaches, and that advertisement's address ends in the locator
- **THEN** the joiner dials the advertised address and pairing succeeds

#### Scenario: Nobody answers
- **WHEN** no candidate completes a handshake within 5 seconds
- **THEN** the joiner reports that the machine could not be reached

#### Scenario: Inviter's mode closed
- **WHEN** the only reachable candidate closes with close reason `4`
- **THEN** the joiner reports that the machine was not ready to pair
