# Proposal

## Why

Copied text already follows the cursor between paired PCs. Copied images don't: a screenshot or a "Copy image" from a browser is reported as "not shared (no text)" and stays behind. Images are the next most common thing people copy, and the brief promises that the clipboard follows the cursor. Files come in a later change of their own, where bytes move when the user pastes.

## What Changes

- **Image copies follow the cursor, as text does.** The design is the same:
  - a copy is announced to every member, without its contents;
  - the newest copy on any PC wins, whether text or image;
  - the contents move only when the cursor crosses into a PC, on a QUIC stream of their own at low priority;
  - nothing is echoed back, and nothing is logged or written to disk.
- **Images travel as PNG.**
  - **When an image is copied:** the session agent takes the copy's PNG if the app put one on the clipboard, as browsers and the Snipping Tool do. Otherwise it encodes the copy's bitmap (DIB) to PNG.
  - **When an image arrives:** the agent sets both PNG and a bitmap, so modern apps and older ones such as Paint can paste it.
- **One kind per copy.** A copy holding both text and an image, as Office often makes, is shared as its text, as today. An image is shared only when the copy has no text.
- **Limits:**
  - an image of at most 32 MiB as PNG and 64 megapixels; anything larger is reported as not shared;
  - text keeps its 4 MiB limit;
  - a fetch now gives up after 10 seconds with no bytes arriving, rather than 10 seconds in total, since a large image can take longer than that on Wi-Fi.
- **The service never decodes an image.** To the SYSTEM service, image bytes are opaque. It checks only their length and the 8-byte PNG signature. Decoding and encoding happen in the session agent, which runs as the user, with a memory-safe PNG library.
- **Version bumps, staying compatible with 0.1.0.**
  - **Peer protocol:** version 2 adds the image stream header. This release speaks versions 1–2. A member connected at version 1 is never offered images and keeps working with text.
  - **IPC:** version 2 adds the image messages between service and agent. This release speaks versions 1–2, so a service and agent mid-upgrade still work, with text only.
- **Docs:** `docs/protocol.md` (new types, version history), `docs/threat-model.md` (untrusted image data, where it's parsed), and the clipboard spec's Purpose.
- **Out of scope:**
  - files, a later change;
  - rich text and HTML;
  - sharing several formats of one copy;
  - images on the secure desktop.

## Capabilities

### New Capabilities

None.

### Modified Capabilities
- `clipboard`: image copies follow the cursor under the existing rules. Text wins when a copy holds both. Images travel as PNG with their own limits, only to members that agreed version 2. The service treats image bytes as opaque. A fetch gives up after 10 seconds without progress.
- `agent-link`: the agent reports image copies and sets received images. Image runs on the agent link have their own start messages and limits, and "not shared" reasons cover a copy with neither text nor an image, and an image over the limits.
- `wire-protocol`: this release speaks peer protocol versions 1 to 2.
- `ipc-protocol`: this release speaks IPC versions 1 to 2.

## Impact

- **Crates touched:**
  - **`proto`:** version 2, `ClipboardImageData` (`0x0015`) as an image stream's first frame, image length and signature checks, `MAX_IMAGE_LEN`.
  - **`transport`:** reads and writes image streams, refuses them on connections agreed at version 1, and reports each connection's agreed version so the service offers images only where they are understood.
  - **`ipc`:** version 2, `ClipboardImage` (agent → service) and `SetClipboardImage` (service → agent), sharing the existing chunk messages, and a "no text or image" reason.
  - **`win32-clipboard`:** reads PNG or DIB from the clipboard and sets PNG plus DIB, with the encoding and decoding as pure functions. It adds a dependency on the `png` crate (MIT/Apache-2.0, pure Rust).
  - **`bin/agent`:** reports images and sets them.
  - **`bin/service`:** the held copy is text or image; fetch, offer and timeout rules; log lines record kind and length.
  - **`tools/fuzz`:** the new decoders and the PNG decode path.
  - **No change:** `engine`, `model`, `pairing`, `policy`, `win32-input`, `win32-session`, `win32-shell`, `apps/desktop`, `installer`.
- **Dependency rules hold:**
  - only `deskhop-agent` links `win32-clipboard`, so the PNG library stays out of the SYSTEM service;
  - no new dependency in `proto`, `model` or `engine`;
  - `proto` and `ipc` bump their versions and update `docs/protocol.md` in this change.
- **No grid, no server/client toggle, no IP address field.**
- **Testing:**
  - unit and transport tests over loopback, including a version 1 peer against a version 2 one;
  - fuzzing of the new decoders;
  - a manual check with the user on two PCs: a screenshot copied on one and pasted in Paint and in a browser on the other.
