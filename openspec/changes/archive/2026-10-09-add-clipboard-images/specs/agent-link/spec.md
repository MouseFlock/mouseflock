# Spec Delta

## MODIFIED Requirements

### Requirement: Clipboard copies are reported
When the session's clipboard changes, other than by the agent itself, the agent SHALL report the change once it settles. Changes within 100 ms count as one. A copy holding text SHALL be reported as `ClipboardText` followed by `ClipboardChunk` frames. A copy holding no text but an image SHALL be reported, on a link that agreed IPC version 2 or later, as `ClipboardImage` followed by `ClipboardChunk` frames carrying the image as PNG. Otherwise the agent SHALL send `ClipboardNotShared` with why: private, no text or image, or too large.

#### Scenario: Text copied
- **WHEN** the user copies `hello` in Notepad
- **THEN** the service end receives `ClipboardText` with length 5 and chunks that together are `hello`

#### Scenario: Image copied
- **WHEN** the user copies an image with no text format, on a link that agreed version 2
- **THEN** the service end receives `ClipboardImage` and chunks that together are a PNG of that image

#### Scenario: Image copied on a version 1 link
- **WHEN** the user copies an image with no text format, on a link that agreed version 1
- **THEN** the service end receives `ClipboardNotShared` with reason no text or image

#### Scenario: Nothing shareable
- **WHEN** the user copies files in Explorer, which puts neither text nor an image on the clipboard
- **THEN** the service end receives `ClipboardNotShared` with reason no text or image

### Requirement: Private copies are recognised
The agent SHALL report a copy as private, without reading its contents, when the clipboard holds the format `ExcludeClipboardContentFromMonitorProcessing`. The same SHALL apply when `CanIncludeInClipboardHistory` or `CanUploadToCloudClipboard` holds the value 0.

#### Scenario: Password manager copy
- **WHEN** an app copies text together with `CanUploadToCloudClipboard` set to 0
- **THEN** the service end receives `ClipboardNotShared` with reason private, and no `ClipboardText`

#### Scenario: Private image
- **WHEN** an app copies an image together with `ExcludeClipboardContentFromMonitorProcessing`
- **THEN** the service end receives `ClipboardNotShared` with reason private, and no `ClipboardImage`

### Requirement: The service sets the clipboard
On `SetClipboard` followed by `SetClipboardChunk` frames, the agent SHALL replace the session's clipboard with that text as Unicode text. On `SetClipboardImage` followed by `SetClipboardChunk` frames, the agent SHALL decode the PNG and replace the session's clipboard with the image, as PNG and as a bitmap; if the PNG cannot be decoded or exceeds the image limits, it SHALL leave the clipboard unchanged and keep the link up. It SHALL NOT report any change it makes as a copy. If the clipboard cannot be opened within 1 second, the agent SHALL leave it unchanged.

#### Scenario: Text set
- **WHEN** the service sends `SetClipboard` with `world`
- **THEN** pasting in Notepad gives `world`, and the service end receives no clipboard report for it

#### Scenario: Image set
- **WHEN** the service sends `SetClipboardImage` with a valid PNG
- **THEN** pasting in Paint shows the image, and the service end receives no clipboard report for it

#### Scenario: Corrupt image
- **WHEN** the service sends `SetClipboardImage` with bytes that begin with the PNG signature but do not decode
- **THEN** the clipboard is unchanged and the link stays up

### Requirement: Clipboard runs on the agent link
A `ClipboardText` or `SetClipboard` SHALL announce a length of at most 4,194,304 bytes, and the text SHALL be valid UTF-8. A `ClipboardImage` or `SetClipboardImage` SHALL be used only on a link that agreed IPC version 2 or later, SHALL announce a length of at most 33,554,432 bytes, and its bytes SHALL begin with the PNG signature. The `ClipboardChunk` or `SetClipboardChunk` frames after a start message SHALL carry exactly the announced number of bytes, each with at least one byte. A chunk outside a run, a run cut short by another clipboard message, or any breach of these limits SHALL be a protocol error.

#### Scenario: Chunk without a run
- **WHEN** the service end receives `ClipboardChunk` without a `ClipboardText` or `ClipboardImage` before it
- **THEN** it closes the agent link

#### Scenario: Input during a run
- **WHEN** the agent sends input events between the chunks of a `ClipboardText` or `ClipboardImage` run
- **THEN** the service end accepts them and the run completes

#### Scenario: Image run on a version 1 link
- **WHEN** a link agreed IPC version 1 and the agent sends `ClipboardImage`
- **THEN** the service end closes the agent link
