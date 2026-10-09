# clipboard Specification

## Purpose

Lets text and images copied on one DeskHop machine be pasted on another. A copy follows the cursor: it is announced to the desk, and its contents move only to the machines the cursor crosses into.

## Requirements

### Requirement: Copies are announced without their text
When the session agent reports a shareable text copy, the service SHALL give it a new random 64-bit ID, keep its text in memory as this machine's held copy, and send `ClipboardOffer` with that ID to every up member. The offer SHALL NOT carry any of the text.

#### Scenario: Copy on A
- **WHEN** A's agent reports that `hello` was copied, and B and C are up members
- **THEN** B and C each receive one `ClipboardOffer` from A, and neither receives the text

### Requirement: The newest copy wins
Each machine SHALL treat the copy it learned of most recently as the newest. That is either its own reported copy or a member's `ClipboardOffer`, in the order the service received them. A later copy SHALL replace an earlier one, whichever machine made either.

#### Scenario: Copies on two machines
- **WHEN** C hears A's offer and then B's offer
- **THEN** C's newest copy is B's

#### Scenario: Own copy after an offer
- **WHEN** C hears A's offer, and then C's own agent reports a copy
- **THEN** C's newest copy is its own, and C fetches nothing from A

### Requirement: Text follows the cursor
When the cursor arrives at this machine, the service SHALL fetch the newest copy if a member made it, this machine does not already hold it, and that member is up. The cursor arrives when focus becomes controlled by a member, or returns here from forwarding. The service SHALL then send `ClipboardRequest` with that copy's ID, and give the received text to the agent to set.

#### Scenario: Copy on A, cross to B
- **WHEN** A's user copies `hello` and the cursor then crosses from A into B
- **THEN** B requests the copy from A, and B's agent is told to set `hello`

#### Scenario: Copy on B, back to A
- **WHEN** the cursor is on B, `world` is copied there, and the cursor crosses back to A
- **THEN** A requests the copy from B, and A's agent is told to set `world`

#### Scenario: Nothing newer
- **WHEN** the cursor crosses into B and B already holds the newest copy
- **THEN** B sends no request

#### Scenario: Owner is down
- **WHEN** the newest copy is A's, A is down, and the cursor arrives at B
- **THEN** B sends no request and keeps its clipboard

### Requirement: Text is sent only on request
The service SHALL send clipboard text only to an up member that sent `ClipboardRequest` for the ID of this machine's held copy. A request for any other ID SHALL be answered with nothing.

#### Scenario: Stale request
- **WHEN** B requests A's earlier copy after A's user copied something newer
- **THEN** A sends B no text

### Requirement: Private and unsupported copies are not shared
A copy the agent reports as not shared SHALL NOT be announced or sent. That covers private copies (marked to be excluded from clipboard monitoring, history or cloud sync), copies with neither text nor an image, text over 4 MiB, and images over the image limits. Such a copy SHALL clear this machine's held copy, so the earlier copy is no longer sent to anyone.

#### Scenario: Password copied
- **WHEN** a password manager on A copies a password marked as excluded from clipboard history
- **THEN** no member receives an offer or text for it

#### Scenario: Private copy after a shareable one
- **WHEN** A's user copies `hello`, then copies a private password, and B then requests `hello`'s ID
- **THEN** A sends B no text

#### Scenario: Private image
- **WHEN** an app copies an image marked as excluded from clipboard history
- **THEN** no member receives an offer or contents for it

### Requirement: Received text is not announced again
Text that this machine's agent sets because a member sent it SHALL NOT be offered again by this machine. This machine SHALL record that it now holds that copy, under the ID its owner gave it.

#### Scenario: No echo
- **WHEN** B receives A's copy and its agent sets it
- **THEN** B sends no `ClipboardOffer`, and when the cursor later returns to A, A fetches nothing

### Requirement: Transfers never delay input
Clipboard contents SHALL travel only on a unidirectional QUIC stream of their own, opened by the sender for one transfer. They SHALL never travel on the control stream. A connection SHALL accept at most one incoming clipboard stream at a time. A machine SHALL give up on a requested copy when 10 seconds pass without any of its bytes arriving, and SHALL discard contents that arrive after a newer copy was learned.

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

### Requirement: Clipboard stream layout and limits
A clipboard stream SHALL carry one header frame, then `ClipboardChunk` frames whose bytes total exactly the header's length, then end. For text the header SHALL be `ClipboardData` (the copy's ID and the text's length in bytes), the length at most 4,194,304 bytes, and the text valid UTF-8. For an image the header SHALL be `ClipboardImageData` (the copy's ID and the PNG's length in bytes), allowed only on a connection that agreed version 2 or later, the length at most 33,554,432 bytes, and the bytes SHALL begin with the PNG signature. A breach SHALL be a protocol error. Contents a machine did not request SHALL be discarded unset.

#### Scenario: Too long
- **WHEN** a clipboard stream's `ClipboardData` claims 5,000,000 bytes
- **THEN** the receiver closes the connection with a protocol error, before reading the text

#### Scenario: Image too long
- **WHEN** a clipboard stream's `ClipboardImageData` claims 40,000,000 bytes
- **THEN** the receiver closes the connection with a protocol error, before reading the image

#### Scenario: Image on a version 1 connection
- **WHEN** a connection agreed version 1 and a clipboard stream starts with `ClipboardImageData`
- **THEN** the receiver closes the connection with a protocol error

#### Scenario: Not a PNG
- **WHEN** an image stream's first eight bytes are not the PNG signature
- **THEN** the receiver closes the connection with a protocol error

#### Scenario: Unrequested text
- **WHEN** B receives a well-formed clipboard stream for an ID it did not request
- **THEN** B discards it and its agent is told nothing

### Requirement: Clipboard messages travel between up members
`ClipboardOffer` and `ClipboardRequest` SHALL be sent only on the control stream of a member connection whose peer is up, and clipboard streams only on such a connection. Either message before the peer is up, any of them on a pairing connection, or on the wrong kind of stream SHALL be a protocol error. A clipboard stream that arrives before the peer's records SHALL be held unread until they arrive, since QUIC does not order streams.

#### Scenario: Offer on a pairing connection
- **WHEN** a pairing connection receives `ClipboardOffer`
- **THEN** the receiver closes it with a protocol error

#### Scenario: Data on the control stream
- **WHEN** a member's control stream carries `ClipboardChunk`
- **THEN** the receiver closes the connection with a protocol error

### Requirement: Clipboard text stays out of logs and off disk
The service SHALL NOT write clipboard text to its log or to any file. It SHALL hold at most this machine's held copy and one incoming transfer per connection in memory. Logs MAY record that a copy was offered, fetched or set, and its length, but never its text.

#### Scenario: Log after a transfer
- **WHEN** `hello` is copied on A and fetched by B
- **THEN** neither service log contains `hello`

### Requirement: Images are shared like text
An image copy SHALL follow every rule in this capability that text does: it is announced without its contents, competes with text for newest copy, is fetched when the cursor arrives, is sent only on request, is not announced again once received, and is kept out of logs and off disk. Where a rule names text, it SHALL apply to an image copy's contents in the same way.

#### Scenario: Screenshot on A, cross to B
- **WHEN** A's user copies a screenshot and the cursor then crosses from A into B
- **THEN** B requests the copy from A, and B's agent is told to set that image

#### Scenario: Text after an image
- **WHEN** A's user copies an image, then copies `hello` on B, and the cursor then crosses into C
- **THEN** C fetches `hello` from B and does not fetch the image

#### Scenario: Image not echoed
- **WHEN** B receives A's image and its agent sets it
- **THEN** B sends no `ClipboardOffer`, and when the cursor later returns to A, A fetches nothing

### Requirement: Text wins when a copy holds both
When a copy holds text, it SHALL be shared as text, whatever else it holds. It SHALL be shared as an image only when it holds no text and holds an image.

#### Scenario: Cells copied from a spreadsheet
- **WHEN** the user copies cells that the app puts on the clipboard as both text and a picture
- **THEN** the copy is shared as its text

#### Scenario: Image copied in a browser
- **WHEN** the user chooses "Copy image" in a browser, which puts no text on the clipboard
- **THEN** the copy is shared as an image

### Requirement: Images travel as PNG
An image copy's contents SHALL travel as one PNG file. A receiving machine SHALL make the image pasteable both by apps that read PNG and by apps that read only a bitmap.

#### Scenario: Pasting in an older app
- **WHEN** B receives A's image and the user pastes in Paint
- **THEN** the image appears in Paint

#### Scenario: Pasting in a browser
- **WHEN** B receives A's image and the user pastes into a web page that accepts images
- **THEN** the page receives a PNG image

### Requirement: Image limits
An image SHALL be shared only if its PNG is at most 33,554,432 bytes (32 MiB) and its width times height is at most 67,108,864 pixels. A larger image SHALL be treated as not shared. A received image beyond either limit SHALL be discarded unset.

#### Scenario: Very large image
- **WHEN** the user copies an image whose PNG would be 40 MiB
- **THEN** no member receives an offer or contents for it

#### Scenario: Received image claims huge dimensions
- **WHEN** B receives a PNG of 10 KB whose header claims 100,000 by 100,000 pixels
- **THEN** B's agent does not set it, and B's clipboard is unchanged

### Requirement: Images only to members that understand them
A machine SHALL announce an image copy only to members whose connection agreed protocol version 2 or later. It SHALL announce text copies to every up member, whatever version their connection agreed.

#### Scenario: Member running 0.1.0
- **WHEN** A runs this release, C runs DeskHop 0.1.0 and agreed version 1, and A's user copies an image
- **THEN** C receives no `ClipboardOffer` for it, C's connection stays up, and text copies still reach C

### Requirement: The service never decodes images
The service SHALL treat an image copy's contents as opaque bytes. It SHALL check only their length and that they begin with the 8-byte PNG signature. Only the session agent SHALL decode or encode images.

#### Scenario: Malformed PNG from a member
- **WHEN** a member sends an image whose bytes begin with the PNG signature but are otherwise corrupt
- **THEN** the service passes them to the agent unparsed, the agent fails to decode them and leaves the clipboard unchanged, and the link and connection stay up
