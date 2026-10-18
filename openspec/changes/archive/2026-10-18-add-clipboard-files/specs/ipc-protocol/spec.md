# Spec Delta

## MODIFIED Requirements

### Requirement: IPC version range of this release
This release SHALL speak IPC versions 1 to 3. Version 2 SHALL differ from version 1 only by adding `ClipboardImage` (`0x001A`, session agent to service) and `SetClipboardImage` (`0x0029`, service to session agent). Version 3 SHALL differ from version 2 only by adding, for file copies, the manifest and per-file content messages in each direction. A link SHALL behave exactly as the older release does whenever it agrees a version below the one that defines a feature.

#### Scenario: Hello sent by a session agent of this release
- **WHEN** a session agent of this release sends `Hello`
- **THEN** the payload is `01 00 03 00 01`

#### Scenario: Agent from the previous release
- **WHEN** the service of this release links with a session agent whose `Hello` advertises IPC versions 1 to 1, as during an upgrade
- **THEN** the link agrees version 1, and input and text clipboard work, with no image or file messages in either direction

#### Scenario: Agent that understands images but not files
- **WHEN** the service of this release links with a session agent whose `Hello` advertises IPC versions 1 to 2, as during an upgrade
- **THEN** the link agrees version 2, input, text and image clipboard work, and no file message travels in either direction
