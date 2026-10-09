# Spec Delta

## MODIFIED Requirements

### Requirement: IPC version range of this release
This release SHALL speak IPC versions 1 and 2. Version 2 SHALL differ from version 1 only by adding `ClipboardImage` (`0x001A`, session agent to service) and `SetClipboardImage` (`0x0029`, service to session agent). A link that agreed version 1 SHALL behave exactly as a version 1 release does.

#### Scenario: Hello sent by a session agent of this release
- **WHEN** a session agent of this release sends `Hello`
- **THEN** the payload is `01 00 02 00 01`

#### Scenario: Agent from the previous release
- **WHEN** the service of this release links with a session agent whose `Hello` advertises IPC versions 1 to 1, as during an upgrade
- **THEN** the link agrees version 1, and input and text clipboard work, with no image messages in either direction
