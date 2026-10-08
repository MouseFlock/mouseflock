# Design

## Context

- **The engine's focus states** (`crates/engine/src/lib.rs`): `Focus::Local`, `Focus::Forwarding { to, exit }`, `Focus::Controlled { by, contest }`. `exit` is the virtual-screen point where the cursor left this machine.
- **Physical input while forwarding** goes through `forward(to, input)`. It already singles out one chord: Ctrl+Alt+End, which sends `PeerMessage::SecureAttention` and swallows the `End` (`forward`, around line 538). Everything else is forwarded as a `PeerMessage::Input` action.
- **Returning home already exists.** `on_undo` (around line 459) does exactly what the escape key needs when the undone edge is the one being forwarded: `forwarded.clear()`, `send(to, PeerMessage::FocusWithdrawn)`, `set_focus(Focus::Local, Some(exit))`. The peer handles `FocusWithdrawn` by releasing held input and taking focus back. None of this waits for a reply.
- **Capture while forwarding is `WithholdAll`**, so every physical key here is already kept from this machine's OS. Nothing new reaches the OS.
- **Scroll Lock** is reported as `Key(0x47)` (HID usage), a normal reported key. It is not one of the special keys (`win32-input` treats only Pause and Num Lock specially). `model` has `Key::END` and the modifier constants but no Scroll Lock constant yet.
- **Tests** are replay tests in `crates/engine/tests/input_focus.rs`, one per spec scenario.

## Goals / Non-Goals

**Goals:**
- The two requirements in `specs/input-focus`.
- Return home without any dependency on the peer, reusing the existing withdraw path.
- Keep the whole change inside `engine` and `model`.

**Non-Goals:**
- Fixing the traps the key rescues (elevated windows, hidden screen).
- A configurable key or any setting.
- Any behavior while `Controlled` (local input already takes focus back there).

## Decisions

### D1. Detect the double press in `forward`, beside the existing chord

`forward(to, input)` gains a branch before the general case: a Scroll Lock press (`Key(0x47)`, `down: true`). It compares `at` with the time of the last Scroll Lock press:
- within `home_gap_ms` → return home (D2), and clear the remembered time;
- otherwise → remember `at` as the last Scroll Lock press.

Either way the function returns without forwarding anything, so Scroll Lock never reaches the peer. A Scroll Lock release (`down: false`) also returns without forwarding. This mirrors how the Ctrl+Alt+End branch swallows its key.

The last-press time lives in a new field on the engine, `last_home_key: Option<Millis>`, cleared whenever focus changes (it only has meaning while forwarding, and `set_focus` already resets per-focus state).

Alternatives:
- **A modifier chord (Ctrl+Alt+Home).** No timing state, and symmetric with Ctrl+Alt+End. But Home is a common key, and a stuck Ctrl or Alt across the crossing makes a chord easy to fire by accident. The double-tap of an unused key is less error-prone. Recorded as the proposal's open choice for the user.
- **A single Scroll Lock press.** One stray press would yank focus home mid-task. The double press is deliberate.

### D2. Return home by the existing withdraw path

Factor the three lines shared with `on_undo` into one helper, `withdraw_to_local(to, exit)`:
```
self.forwarded.clear();
self.send(to, PeerMessage::FocusWithdrawn);
self.set_focus(Focus::Local, Some(exit));
```
`on_undo` calls it, and the escape-key branch calls it with the current `Forwarding { to, exit }`. `set_focus(Focus::Local, Some(exit))` emits the warp back to the exit point, the capture change to `PassAll`, and `FocusChanged(Local)`, exactly as a normal return. The engine holds no key state for its own forwarded input (the peer releases what it held on `FocusWithdrawn`), so nothing extra must be released here.

### D3. The double-press window is tuning

`Tuning` gains `home_gap_ms: u64`, default `500`, beside `burst_gap_ms` and the others. Replay tests can set it. 500 ms matches a deliberate double tap without being so long that two separate presses count.

### D4. Scroll Lock constant in `model`

`model::Key` gains `pub const SCROLL_LOCK: Key = Key(0x47);`, used by the engine. A constant only; no behavior in `model`.

## Risks / Trade-offs

- **[Scroll Lock no longer reaches a peer]** → Scroll Lock is almost never needed on a controlled PC, and Ctrl+Alt+End already swallows `End` the same way. Accepted, and stated in the spec.
- **[A user with no Scroll Lock key]** (some compact and laptop keyboards) → Those users can't use the default trigger. This is why the key choice is open for the user at review; a chord fallback is the alternative.
- **[The key fires while the double-tap was meant for the peer]** → Scroll Lock isn't forwarded regardless, so there is nothing to mean for the peer. No ambiguity.

## Open Questions

None. The trigger key is a user choice recorded in the proposal, not a deferred unknown: whichever is chosen, the approach, specs and tasks are the same bar the key constant and the timing.
