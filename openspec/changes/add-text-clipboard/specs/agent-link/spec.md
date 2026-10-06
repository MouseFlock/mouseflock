# Spec Delta

## ADDED Requirements

### Requirement: Clipboard copies are reported
When the session's clipboard changes, other than by the agent itself, the agent SHALL report the change once it settles. Changes within 100 ms count as one. The report SHALL be the copied text, as `ClipboardText` followed by `ClipboardChunk` frames, or `ClipboardNotShared` with why: private, no text, or text over 4 MiB.

#### Scenario: Text copied
- **WHEN** the user copies `hello` in Notepad
- **THEN** the service end receives `ClipboardText` with length 5 and chunks that together are `hello`

#### Scenario: Image copied
- **WHEN** the user copies an image with no text format
- **THEN** the service end receives `ClipboardNotShared` with reason no text

### Requirement: Private copies are recognised
The agent SHALL report a copy as private, without reading its text, when the clipboard holds the format `ExcludeClipboardContentFromMonitorProcessing`. The same SHALL apply when `CanIncludeInClipboardHistory` or `CanUploadToCloudClipboard` holds the value 0.

#### Scenario: Password manager copy
- **WHEN** an app copies text together with `CanUploadToCloudClipboard` set to 0
- **THEN** the service end receives `ClipboardNotShared` with reason private, and no `ClipboardText`

### Requirement: The service sets the clipboard
On `SetClipboard` followed by `SetClipboardChunk` frames, the agent SHALL replace the session's clipboard with that text as Unicode text. It SHALL NOT report that change as a copy. If the clipboard cannot be opened within 1 second, the agent SHALL leave it unchanged.

#### Scenario: Text set
- **WHEN** the service sends `SetClipboard` with `world`
- **THEN** pasting in Notepad gives `world`, and the service end receives no clipboard report for it

### Requirement: Clipboard runs on the agent link
A `ClipboardText` or `SetClipboard` SHALL announce a length of at most 4,194,304 bytes. The `ClipboardChunk` or `SetClipboardChunk` frames after it SHALL carry exactly that many bytes, each with at least one byte, and the text SHALL be valid UTF-8. A chunk outside a run, a run cut short by another clipboard message, or any breach of these limits SHALL be a protocol error.

#### Scenario: Chunk without a run
- **WHEN** the service end receives `ClipboardChunk` without a `ClipboardText` before it
- **THEN** it closes the agent link

#### Scenario: Input during a run
- **WHEN** the agent sends input events between the chunks of a `ClipboardText` run
- **THEN** the service end accepts them and the run completes
