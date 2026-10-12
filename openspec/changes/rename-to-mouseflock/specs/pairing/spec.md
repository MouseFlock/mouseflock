## MODIFIED Requirements

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
