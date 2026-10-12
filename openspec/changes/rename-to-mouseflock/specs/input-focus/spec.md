## MODIFIED Requirements

### Requirement: Physical and injected input are distinguished
Every local input event SHALL be known to be either physical (from a device on this machine) or injected (produced by MouseFlock on behalf of a peer, or by other software). Only physical input SHALL count as the local user acting: injected input SHALL never start a takeover or be forwarded to a peer.

#### Scenario: Injected motion does not take over
- **WHEN** peer A controls this machine and MouseFlock injects A's motion here
- **THEN** that motion does not count as local activity and does not start a takeover
