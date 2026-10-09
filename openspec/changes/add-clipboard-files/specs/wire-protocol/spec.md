# Spec Delta

## MODIFIED Requirements

### Requirement: Protocol version range of this release
This release SHALL speak protocol versions 1 to 3, advertising a lowest version of 1 and a highest version of 3 in its `Hello`. Version 2 SHALL differ from version 1 only by adding the message type `0x0015` `ClipboardImageData`, allowed on clipboard streams. Version 3 SHALL differ from version 2 only by adding, for file copies, `0x0016` `ClipboardFilesData` (a manifest, on clipboard streams), `0x0017` `FileContentRequest` (on the control stream) and `0x0018` `FileContentData` (one file's contents, on clipboard streams). A connection SHALL behave exactly as the older release does whenever it agrees a version below the one that defines a feature.

#### Scenario: Hello sent by this release
- **WHEN** this release sends `Hello`
- **THEN** the payload is `01 00 03 00`

#### Scenario: Peer running 0.1.0
- **WHEN** this release connects to a peer whose `Hello` advertises versions 1 to 1
- **THEN** the connection agrees version 1, and pairing, input and text clipboard work as between two version 1 releases

#### Scenario: Peer that understands images but not files
- **WHEN** this release connects to a peer whose `Hello` advertises versions 1 to 2
- **THEN** the connection agrees version 2, text and image clipboard work, and no file copy is offered on it
