# Spec Delta

## ADDED Requirements

### Requirement: Bringing input home from the keyboard
While this machine's input has focus on a peer, pressing Scroll Lock twice within 500 ms on this machine SHALL return focus to this machine locally, with the cursor at the point where it left, and SHALL release every key and mouse button held for that peer. This SHALL take effect from this machine alone, without waiting for the peer. The peer SHALL be told focus was withdrawn, so it releases what it held and takes focus back, but the return home SHALL NOT depend on that message being answered.

#### Scenario: Peer is unresponsive
- **WHEN** this machine's input is forwarded to peer B, B is injecting nothing back and its cursor is not moving, and the user presses Scroll Lock twice within 500 ms
- **THEN** this machine has focus locally with its cursor where it left, and its physical keyboard and mouse reach this machine again

#### Scenario: Held keys released on return
- **WHEN** the user holds Ctrl, the cursor is forwarding to B, and the user presses Scroll Lock twice within 500 ms
- **THEN** focus is local on this machine and B is told to release what it held for this machine, including Ctrl

#### Scenario: Too slow is not a trigger
- **WHEN** this machine is forwarding to B and the user presses Scroll Lock, then presses it again 800 ms later
- **THEN** focus stays on B, and neither press reaches this machine's OS or B

### Requirement: Scroll Lock is withheld from the peer while forwarding
While this machine's input has focus on a peer, Scroll Lock presses and releases SHALL be withheld from this machine's OS and SHALL NOT be forwarded to the peer, so Scroll Lock never toggles on the peer. While focus is local, Scroll Lock SHALL reach this machine's OS unchanged.

#### Scenario: Scroll Lock not sent to the peer
- **WHEN** this machine is forwarding to B and the user presses Scroll Lock once
- **THEN** B does not receive a Scroll Lock press, and this machine's OS does not either

#### Scenario: Scroll Lock works locally
- **WHEN** focus is local and the user presses Scroll Lock
- **THEN** the key reaches this machine's OS and nothing is sent to any peer
