# Tasks

## 1. The escape key in the engine (`model`, `engine`)

- [x] 1.1 Add `model::Key::SCROLL_LOCK` (`0x47`). Verify it compiles and `cargo test -p model` passes.
- [x] 1.2 Add `Tuning::home_gap_ms`, default `500` (D3). Verify `Tuning::DEFAULT` still builds and existing engine tests pass.
- [x] 1.3 Factor the return-home steps out of `on_undo` into `withdraw_to_local(to, exit)` (D2), and have `on_undo` call it. Verify the existing `undo_a_learned_edge` replay test still passes unchanged.
- [x] 1.4 In `forward`, add the Scroll Lock branch (D1): a press within `home_gap_ms` of the last calls `withdraw_to_local` with the current `Forwarding { to, exit }`; otherwise it records the press time; a release is swallowed. Never forward Scroll Lock. Track `last_home_key`, cleared on focus change. Verify with replay tests, one per `specs/input-focus` scenario:
  - **Peer is unresponsive:** forwarding to B, two Scroll Lock presses within 500 ms give `FocusWithdrawn` to B, a warp to the exit point, `SetCapture(PassAll)` and `FocusChanged(Local)`; focus is local.
  - **Held keys released on return:** Ctrl held, then the double press, sends `FocusWithdrawn` to B and ends local; (the peer's own release of Ctrl is `FocusWithdrawn`'s contract, already tested).
  - **Too slow is not a trigger:** two presses 800 ms apart leave focus on B and send nothing to B or the OS.
  - **Scroll Lock not sent to the peer:** one press while forwarding sends no `Input` to B.
  - **Scroll Lock works locally:** with focus local, a Scroll Lock press is quiet (not withheld, not sent) — the engine reports no decision, so the OS gets it.
  - A double press while `Controlled` or `Local` does nothing.

## 2. Docs

- [x] 2.1 Update `docs/known-issues.md`: in both issues' "how to get out", name the key (Scroll Lock twice) as the way home from the controlling PC's own keyboard, keeping the controlled-PC recovery as the alternative. Verify the wording matches the shipped trigger from task 1.4.
