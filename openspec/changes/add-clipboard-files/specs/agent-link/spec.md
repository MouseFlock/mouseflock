# Spec Delta

## ADDED Requirements

### Requirement: A file copy is reported as a manifest
When the session's clipboard holds copied files and no shareable text or image, the session agent SHALL report the copy, on a link that agreed IPC version 3 or later, as a file manifest: each entry's path relative to the copy root, whether it is a directory, and each file's size. It SHALL NOT read any file's contents to report the copy. On an earlier link it SHALL report the copy as not shared, no text or image.

#### Scenario: Files copied
- **WHEN** the user copies two files and a folder, on a link that agreed version 3
- **THEN** the service end receives a file manifest listing the files and the folder, with sizes, and no file contents

#### Scenario: Files copied on a version 1 or 2 link
- **WHEN** the user copies files on a link that agreed version 2
- **THEN** the service end receives a not-shared report, not a manifest

### Requirement: The agent serves a file's contents on request
On a request from the service for a file in the copy the agent currently holds as its own, named by its manifest index, the session agent SHALL stream that file's contents to the service. A request for an index outside the held copy, or for a copy it no longer holds, SHALL be answered with nothing. The agent SHALL stream from disk rather than reading the whole file into memory.

#### Scenario: Content requested
- **WHEN** the service asks the agent for file index 2 of the copy it holds
- **THEN** the agent streams that file's bytes to the service

#### Scenario: Stale content request
- **WHEN** the service asks for a file of a copy the agent no longer holds
- **THEN** the agent sends no contents

### Requirement: The service sets a file copy for paste
On a message giving the agent a file manifest to offer, the session agent SHALL place a copy on the clipboard that offers those files for paste without their bytes, using Windows' delayed rendering. When the file manager pastes and asks for a file's contents, the agent SHALL request that file from the service and stream the received bytes to the file manager. The agent SHALL NOT report any copy it places this way. A winlogon agent's link SHALL never carry a file manifest, in either direction.

#### Scenario: Offer placed, paste served
- **WHEN** the service sends the agent a manifest of one file and the user then pastes in Explorer
- **THEN** the agent offers the file, requests its contents from the service on paste, and the file is written, with no copy reported back

#### Scenario: Manifest on a winlogon link
- **WHEN** a manifest is sent on a winlogon agent's link, in either direction
- **THEN** the link is closed with a protocol error

### Requirement: File runs on the agent link
A file manifest and a file content run SHALL be used only on a link that agreed IPC version 3 or later. A content run SHALL carry one file's bytes, in file chunk frames of its own whose total matches the length its start announced, which MAY exceed the text and image limits. The side sending a file's bytes SHALL keep at most 1 MiB of them unacknowledged by the other side. A chunk outside a run, a run cut short, more than 1 MiB unacknowledged, or a file manifest or content message on a version 1 or 2 link SHALL be a protocol error.

#### Scenario: Content run on a version 2 link
- **WHEN** a link agreed IPC version 2 and a file manifest or content message is sent
- **THEN** the receiver closes the link

#### Scenario: Input during a content run
- **WHEN** input events arrive between the chunks of a file content run
- **THEN** the receiver accepts them and the run completes

#### Scenario: A copy during a content run
- **WHEN** the session agent reports a text copy while it is serving a file
- **THEN** the service end receives the copy and the file run continues

#### Scenario: Past the window
- **WHEN** the agent sends more than 1 MiB of a file's bytes that the service has not acknowledged
- **THEN** the service end closes the agent link
