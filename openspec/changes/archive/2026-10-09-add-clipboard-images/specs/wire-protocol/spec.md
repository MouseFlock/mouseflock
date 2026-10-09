# Spec Delta

## MODIFIED Requirements

### Requirement: Protocol version range of this release
This release SHALL speak protocol versions 1 and 2, advertising a lowest version of 1 and a highest version of 2 in its `Hello`. Version 2 SHALL differ from version 1 only by adding the message type `0x0015` `ClipboardImageData`, allowed on clipboard streams. A connection that agreed version 1 SHALL behave exactly as a version 1 release does.

#### Scenario: Hello sent by this release
- **WHEN** this release sends `Hello`
- **THEN** the payload is `01 00 02 00`

#### Scenario: Peer running 0.1.0
- **WHEN** this release connects to a peer whose `Hello` advertises versions 1 to 1
- **THEN** the connection agrees version 1, and pairing, input and text clipboard work as between two version 1 releases
