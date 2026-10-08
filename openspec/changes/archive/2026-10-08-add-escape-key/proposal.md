# Proposal

## Why

While you control another PC, your own keyboard and mouse are withheld here and forwarded there. Getting them back normally means steering the cursor to the edge you came in by. When the other PC stops responding, that way home is gone and your keyboard and mouse seem frozen. The two-PC check on 2026-10-08 hit this three ways, now in `docs/known-issues.md`:

- an elevated window in front on the controlled PC (Windows drops injected input);
- the controlled PC's screen not visible, behind a hardware KVM switch;
- the controlled PC's agent hung.

In each case the only way out is the controlled PC's own keyboard or mouse. There is no way to bail out from the keyboard you are actually holding.

## What Changes

- **A key that always brings your input home.** While this machine's input is forwarded to a peer, pressing **Scroll Lock twice** within half a second returns focus to this machine, at the point the cursor left, and releases anything held for the peer. It does this from the engine alone, so it never depends on the peer answering.

  *(Scroll Lock twice is the proposed trigger: the traditional KVM key, rarely used, and a double press to avoid accidents. Say so at review if you would rather use a chord such as Ctrl+Alt+Home.)*
- **Only while forwarding.** With focus local, Scroll Lock reaches this machine's OS unchanged, so Scroll Lock still works normally. While forwarding, Scroll Lock is withheld from the peer and never toggles it, like the `End` of Ctrl+Alt+End today.
- **Docs:** `docs/known-issues.md`'s "how to get out" lines point at the key as the reliable way home.
- **Out of scope:**
  - fixing the underlying traps (elevated windows, the hidden-screen case): this is the safety net, not the fix;
  - a configurable key, or any UI for it;
  - any change while this machine is being controlled (local input already takes control back).

## Capabilities

### New Capabilities

None.

### Modified Capabilities
- `input-focus`: a reserved key pressed twice while forwarding returns focus to this machine locally, at the exit point, releasing held input, without any message needing a reply from the peer. The key is withheld from the peer while forwarding and reaches the OS while focus is local.

## Impact

- **Crates touched:**
  - **`model`:** a `Key` constant for Scroll Lock (`0x47`).
  - **`engine`:** detect the double press while `Forwarding`, and return home by the existing withdraw path. A new `Tuning` field for the double-press window.
  - **No change:** `proto`, `ipc`, `transport`, `pairing`, `policy`, `win32-*`, `bin/*`, `apps/desktop`, `installer`. Scroll Lock is already captured and reported; the engine already withholds all physical input while forwarding, so nothing new reaches the OS.
- **Dependency rules hold:** the change is confined to `engine` and `model`, both Linux-testable. No new dependency in `proto`, `model` or `engine`. No protocol change, so no version bump.
- **No grid, no server/client toggle, no IP address field.**
- **Testing:** replay tests in `engine` (model events in, decisions out), as the engine rules require. The behavior is checked for real in the deferred two-PC pass, where the traps it rescues live.
