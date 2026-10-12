# ADR 0005 — Layout learned by use; optional arrangement view after MVP

**Status:** Accepted (2026-09-29)
**Date:** 2026-09-28
**Refines:** ADR 0004 (Layout)

## Context

ADR 0004 makes layout a graph of edge→peer transitions learned by use, and the design brief rules out a Synergy-style screen grid. Two gaps remain:

- Multiple monitors. A machine's virtual screen is the union of its monitors and is often irregular. ADR 0004 says machines "expose their virtual-screen edges" without saying at what granularity, or what happens when a laptop docks and undocks.
- Visibility. Learning alone gives the user no way to see what was learned, correct a wrong answer, or place two peers on the same side of one machine.

A GUI arrangement (drag machines and monitors into place, as Synergy does) solves visibility and precision. As a required setup step it is the configuration burden the product exists to remove.

## Decision

- **Learned first.** Layout is learned by use and MouseFlock works without the user ever arranging anything.
- **Optional arrangement view, post-MVP.** A later UI page shows the learned layout and lets the user correct it by dragging. It edits the same graph learning writes. It is never required, and nothing prompts the user to open it.
- **Slots per monitor side.** The graph maps each outer side of each monitor to at most one peer. The MVP fills every monitor on one side of the machine at once, so the user only ever answers per machine side; the view can later split them.
- **Saved per monitor set.** Each set of connected monitors (docked, undocked) keeps its own layout, as Windows display settings do. A new monitor set starts from the sides learned for the previous one.
- **Monitor identity is opaque to `model`.** The agent supplies it. Until testing shows which is stable across ports and docks, the agent records both the EDID identity (manufacturer, product, serial) and the Windows device path.

Crossing behavior, first-crossing placement, and the learning prompt are specified in `openspec/`.

## Alternatives considered

- **Setup grid** (Synergy). Precise; rejected because nothing works until the user fills it in.
- **Learned only, no view.** Simplest; rejected because a wrong answer cannot be seen or corrected and stacked peers cannot be expressed.
- **Four slots per machine** (left, right, top, bottom). Simplest model and survives docking; rejected because the view would need a different model and learned layouts would have to be migrated.
- **Slots per edge segment.** Most expressive; rejected because editing arbitrary segments is a grid by another name.

## Consequences

- The design brief's non-goal is reworded: no setup grid, ever; an optional view of the learned layout is allowed.
- `model` carries monitor-side slots and monitor-set keys from the start, a little more than the MVP strictly needs.
- A spike in `win32-input` decides which monitor identity is used; monitors with missing or duplicate EDID serials need a fallback.
- The arrangement view is the only layout UI. A feature that needs anything more is still designed wrong.
