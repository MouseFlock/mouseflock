# Spec Delta

## MODIFIED Requirements

### Requirement: The engine drives the agent and peers
The service SHALL feed the engine with the active agent's input in service-clock time, its screen reports, peer messages, and peers coming up or going down. It SHALL carry out every decision: capture mode to every linked agent, injection and warp to the active agent, messages to peers, and layouts to storage. The active agent is the one on the input desktop (`secure-desktop`). A decision for an agent while none is linked SHALL be dropped.

#### Scenario: The cursor crosses
- **WHEN** A and B are paired and up, A learned that B is to its right, and the user pushes A's cursor through A's right edge
- **THEN** A's agent withholds all physical input, and B's agent receives a warp to B's left edge followed by the user's injected movement

#### Scenario: Peer goes down while controlling
- **WHEN** A controls B and A's service stops
- **THEN** B's engine learns that A is down and B's agent stops withholding the mouse

### Requirement: Orderly stop
On a stop request (the Service Control Manager stopping the service, system shutdown, `quit`, or Ctrl+C in console mode), the service SHALL say goodbye to every peer, end every agent link, and finish pending writes before exiting. Ending the links SHALL make every agent stop withholding input.

#### Scenario: Stopped while forwarding
- **WHEN** A's input has focus on B and A's service is stopped
- **THEN** B reports A down as having said goodbye and releases what A held, A's agents exit, and A's keyboard and mouse work locally

#### Scenario: Stopped at the lock screen
- **WHEN** A is locked, its input has focus on B, and A's service is stopped
- **THEN** A's winlogon agent stops withholding and exits, and the user can type A's password at A's own keyboard
