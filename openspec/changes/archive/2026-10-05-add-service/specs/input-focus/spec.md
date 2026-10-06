# Spec Delta

## ADDED Requirements

### Requirement: Losing local input
When this machine's input agent ends, focus that depends on it SHALL be returned and held input released. While this machine's input had focus on a peer, the peer SHALL release everything it held for this machine and take focus back. While a peer controlled this machine, the peer SHALL get focus back as if this machine's user had taken control. Focus here SHALL become local.

#### Scenario: Agent ends while forwarding
- **WHEN** focus is on peer B, the user holds Ctrl, and this machine's agent ends
- **THEN** B releases Ctrl and takes focus back, and this machine's focus is local

#### Scenario: Agent ends while controlled
- **WHEN** A controls this machine and this machine's agent ends
- **THEN** A gets focus back locally with its cursor where it left, and this machine's focus is local

#### Scenario: Agent ends with local focus
- **WHEN** focus is local and this machine's agent ends
- **THEN** nothing is sent to any peer
