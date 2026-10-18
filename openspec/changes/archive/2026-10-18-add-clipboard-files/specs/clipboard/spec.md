# Spec Delta

## MODIFIED Requirements

### Requirement: Transfers never delay input
Clipboard contents SHALL travel only on a unidirectional QUIC stream of their own, opened by the sender for one transfer. They SHALL never travel on the control stream. A connection SHALL accept at most one incoming clipboard stream carrying a copy (text, an image, or a file copy's manifest) at a time and, on a connection that agreed a version defining files, at most one carrying a file's contents at a time; a second stream of either kind that arrives while one of its kind is open SHALL be left unread until the first is done. A sender SHALL open at most one stream of each kind at a time on a connection. A machine SHALL give up on a requested copy when 10 seconds pass without any of its bytes arriving, and SHALL discard contents that arrive after a newer copy was learned.

#### Scenario: Typing during a large transfer
- **WHEN** a 32 MiB image is being transferred from A to B while A forwards keystrokes to B
- **THEN** the keystrokes arrive without waiting for the transfer to finish

#### Scenario: Newer copy during a transfer
- **WHEN** B is receiving A's copy and B's own user copies something before it finishes
- **THEN** B does not set A's copy when it arrives

#### Scenario: Slow but steady transfer
- **WHEN** a large image takes 25 seconds to arrive, with bytes arriving at least every few seconds
- **THEN** B sets it when it completes

#### Scenario: Transfer stalls
- **WHEN** B requested a copy and no bytes of it arrive for 10 seconds
- **THEN** B gives up, and a later arrival of the cursor may request it again

#### Scenario: Text copied during a file paste
- **WHEN** B is pasting a large file owned by A, and A's user copies `hello` and the cursor then crosses into B
- **THEN** B fetches `hello` from A while the file is still arriving, and both complete

#### Scenario: Two copies at once
- **WHEN** a member opens a second stream carrying a copy while its first is still open on the same connection
- **THEN** the receiver reads the second only once the first is done, and both complete
