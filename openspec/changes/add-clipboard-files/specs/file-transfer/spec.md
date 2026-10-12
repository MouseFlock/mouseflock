# Spec Delta

## Purpose

Lets files copied on one MouseFlock machine be pasted on another. A file copy follows the cursor under the clipboard's announce, newest-copy and request model, but its bytes move only when the user pastes, pulled file by file, so nothing large moves unless it is wanted.

## ADDED Requirements

### Requirement: A file copy is shared like any clipboard copy
When the session agent reports that files were copied, the service SHALL treat it as a clipboard copy under the `clipboard` capability: it is given a new random ID, becomes this machine's held copy, is announced to every up member with an offer that carries no contents, and competes with text and image copies for the newest copy. A file copy SHALL be offered only to members whose connection agreed a protocol version that defines files.

#### Scenario: Files copied on A
- **WHEN** A's user copies two files in Explorer, and B and C are up members at a version that defines files
- **THEN** B and C each receive one clipboard offer from A, carrying no file contents or names

#### Scenario: Member that does not understand files
- **WHEN** A copies files, and C's connection agreed a version before files
- **THEN** C receives no offer for that copy, C's connection stays up, and text and image copies still reach C

### Requirement: The manifest arrives with the cursor, the bytes on paste
When the cursor arrives at a machine and the newest copy is a file copy an up member holds that this machine does not, the machine SHALL fetch only the copy's manifest: each file's path relative to the copy root, whether it is a directory, and each file's size in bytes. It SHALL NOT fetch any file's contents then. Using the manifest, the agent SHALL place a copy on this machine's clipboard that offers the files for paste without their bytes. Each file's bytes SHALL be fetched from the owning member only when the user pastes.

#### Scenario: Cursor arrives, nothing large moves
- **WHEN** A holds a copy of a 2 GiB file and the cursor crosses into B
- **THEN** B fetches the manifest, offers the file for paste, and no file bytes have moved

#### Scenario: Paste pulls the bytes
- **WHEN** B holds that offer and the user pastes into a folder in Explorer
- **THEN** B requests the file's contents from A, streams them, and Explorer writes the 2 GiB file into that folder

#### Scenario: Never pasted, never moved
- **WHEN** the cursor crosses into B and the user copies something else without pasting
- **THEN** no file bytes were ever fetched from A

### Requirement: Multiple files and directory trees
A file copy MAY hold several files and whole directory trees. The manifest SHALL list every file in each tree by its path relative to the copy root, and SHALL list each directory, so empty directories are recreated. On paste, the receiving machine SHALL recreate the tree under the paste target and fetch each file's contents.

#### Scenario: A folder with subfolders
- **WHEN** A copies a folder containing files and an empty subfolder, and the user pastes on B
- **THEN** the folder, its files and the empty subfolder appear under the paste target with the same structure

### Requirement: No small size cap, streamed per file
A file's contents SHALL travel on a stream of its own, read from the source and written to the target as they arrive, so a file larger than the text and image limits is carried. One transfer SHALL move one file's contents.

#### Scenario: A large file
- **WHEN** a 5 GiB file is pasted from A to B
- **THEN** it is transferred and written without being held whole in memory on either machine

#### Scenario: Typing during a transfer
- **WHEN** a large file is being pasted from A to B while A forwards keystrokes to B
- **THEN** the keystrokes arrive without waiting for the transfer

### Requirement: Copies that are not shared
A copy with no files SHALL NOT be shared as a file copy. To bound a hostile manifest, a copy whose manifest lists more than 65,536 entries, or whose files total more than 68,719,476,736 bytes (64 GiB), or that holds an entry whose path relative to the copy root is longer than 259 UTF-16 code units, SHALL NOT be shared, as a whole. The winlogon agent SHALL never report or set a file copy.

#### Scenario: Empty selection
- **WHEN** nothing with files is on the clipboard
- **THEN** no file copy is announced

#### Scenario: Manifest over the ceiling
- **WHEN** a received manifest claims more than 65,536 entries or more than 64 GiB in total
- **THEN** the receiver rejects it and offers nothing to paste

#### Scenario: A path too long to paste
- **WHEN** A's user copies a folder in which one file's path, relative to the copied folder, is 300 characters long
- **THEN** no member receives an offer for that copy, and a received manifest with such an entry is rejected whole

### Requirement: Arriving files stay under the paste target
A received file SHALL be written only within the folder the user pastes into. A manifest path that is absolute, names a drive, or climbs above the copy root with `..` SHALL be rejected, and the copy SHALL NOT be pasted. Name collisions with existing files SHALL be resolved by the file manager as for any paste, never by overwriting outside the target.

#### Scenario: Path traversal in a manifest
- **WHEN** a member sends a manifest with an entry whose relative path is `..\\..\\Windows\\System32\\evil.dll`
- **THEN** the receiver rejects the manifest and writes nothing

#### Scenario: Absolute path in a manifest
- **WHEN** a manifest entry's path is `C:\\Windows\\x.dll`
- **THEN** the receiver rejects the manifest and writes nothing

### Requirement: The service never opens a file
The service SHALL treat a file's contents as opaque bytes it relays between the receiving member and the owning member. It SHALL NOT open, read or write any file. Only the session agent, running as the user, SHALL read source files and hand received bytes to the file manager.

#### Scenario: Service relays bytes
- **WHEN** B pastes a file owned by A
- **THEN** A's service reads nothing from disk itself; A's agent reads the source file and the service relays its bytes to B, whose agent gives them to Explorer

### Requirement: The owning member goes away mid-paste
If the owning member goes down, or the file cannot be read, while its contents are being pasted, the paste SHALL fail rather than write a truncated file silently, and the link and connections SHALL stay up.

#### Scenario: Owner lost during paste
- **WHEN** B is pasting a file owned by A and A goes down before the file finishes
- **THEN** Explorer reports the paste failed, and B's and A's connections stay up

### Requirement: File names and bytes stay out of logs and off the service
The service SHALL NOT write a file's name, path or bytes to its log or to any file. Logs MAY record that a file copy was offered, fetched or set, and the file count and total size, but never a name or any file's bytes.

#### Scenario: Log after a file paste
- **WHEN** a file named `salary.xlsx` is pasted from A to B
- **THEN** neither service log contains `salary.xlsx` or the file's bytes
