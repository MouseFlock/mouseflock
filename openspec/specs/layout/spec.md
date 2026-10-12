# layout Specification

## Purpose

Learns which peer lies beyond each edge of a machine's screens from how the user moves the cursor, so that MouseFlock works without anyone arranging screens, and lets the user take back a wrong guess.

## Requirements

### Requirement: Learning an edge by pushing through it
When the cursor pushes outward through an outer monitor side that maps to no peer, this machine SHALL learn that side and cross to the peer when all of these hold: exactly one connected peer is unplaced (it appears on no side of this machine's layout for the current monitor set); at least 20 device units of outward movement are pushed against the side with no pause longer than 250 ms, whatever the edge sensitivity setting; the cursor is outside the corner zones; and no mouse button is held. The crossing that follows SHALL behave as any other crossing.

#### Scenario: First push reaches the only unplaced peer
- **WHEN** B is the only connected peer, no side of this machine maps to B, and the user pushes 25 units out of the right side of the only monitor
- **THEN** the right side maps to B and focus moves to B

#### Scenario: A brush against the edge learns nothing
- **WHEN** B is the only unplaced peer and the user moves 8 units out of the right side, then stops
- **THEN** the layout is unchanged and focus stays local

#### Scenario: Instant sensitivity still needs a push to learn
- **WHEN** the edge sensitivity is `instant`, B is the only unplaced peer, and the user pushes 5 units out of an unmapped side
- **THEN** nothing is learned and focus stays local

### Requirement: Learning fills the whole machine side
Learning a side SHALL map that side of every monitor in the current set whose side on that direction is at least partly outer, not only the monitor the cursor left.

#### Scenario: Two monitors stacked
- **WHEN** a machine has two monitors stacked vertically and learns its right side while the cursor is on the upper one
- **THEN** the right sides of both monitors map to the learned peer

#### Scenario: Taller monitor beside a shorter one
- **WHEN** a taller monitor sits left of a shorter one, and the machine learns its right side
- **THEN** the right side of the shorter monitor maps to the peer, and so does the right side of the taller monitor, because it is outer above the shorter one

### Requirement: No guess without a single candidate
An unmapped outer side SHALL stay a wall when no connected peer is unplaced, or when more than one is.

#### Scenario: Every peer already placed
- **WHEN** B is the only connected peer and the right side already maps to B
- **THEN** pushing through the unmapped left side learns nothing and does not cross

#### Scenario: Two unplaced peers
- **WHEN** B and C are connected and neither is placed on this machine
- **THEN** pushing through an unmapped side learns nothing and does not cross

### Requirement: Learning the reverse edge
When focus enters this machine through one of its sides from a peer's edge, and that peer is unplaced on this machine and every monitor side of this machine on that direction that is at least partly outer maps to no peer, this machine SHALL learn that side toward the peer whose edge the cursor left. It SHALL NOT offer Undo for a reverse edge.

#### Scenario: Both sides from one push
- **WHEN** A learns its right side toward B and the cursor enters B through B's left side
- **THEN** B's left side maps to A

#### Scenario: Reverse edge through a middle machine
- **WHEN** A controls B, the cursor leaves B's right side into C, and C is entered through its left side
- **THEN** C's left side maps to B, not to A

#### Scenario: Existing side is kept
- **WHEN** the cursor enters B through B's left side from A, but B's left side already maps to C
- **THEN** B's layout is unchanged

### Requirement: Undo a learned edge
After learning a side, this machine SHALL offer Undo naming the peer and the side for 10 seconds. Undo within that time SHALL remove the learned side from this machine's layout and SHALL tell the peer to remove the side it maps to this machine. If this machine's input has focus on that peer through the crossing that learned the side, focus SHALL return to this machine with the cursor where it left, releasing held input as for any focus change. Undo after 10 seconds SHALL do nothing.

#### Scenario: Undo right after learning
- **WHEN** this machine learns its right side toward B, crosses, and the user chooses Undo 3 s later
- **THEN** the right side maps to no peer, B's left side no longer maps to this machine, and focus is back on this machine at the exit point

#### Scenario: Undo too late
- **WHEN** the user chooses Undo 11 s after the side was learned
- **THEN** the layout is unchanged

### Requirement: Forget layout
Forget layout SHALL clear this machine's learned sides for every monitor set. Other machines' layouts SHALL be unchanged. The next push through an unmapped side SHALL learn again.

#### Scenario: Relearn after forgetting
- **WHEN** this machine's right side maps to B, the user chooses Forget layout, and then pushes 25 units out of the left side
- **THEN** the left side maps to B and the right side maps to no peer

### Requirement: Layouts per monitor set
This machine SHALL keep a separate layout for each set of connected monitors. When the monitor set changes to one seen before, its own layout SHALL apply. When it changes to a set not seen before, the new set's layout SHALL start with each side mapped to the peer that side mapped to in the previous set, applied to every monitor side that is at least partly outer on that direction.

#### Scenario: Docking a laptop
- **WHEN** a laptop whose right side maps to B is docked to an external monitor to its right
- **THEN** in the new monitor set, the external monitor's right side maps to B

#### Scenario: Undocking returns the old layout
- **WHEN** the docked set's layout is changed and the laptop is then undocked
- **THEN** the undocked set's own earlier layout applies

### Requirement: Layout changes are reported
Every change to this machine's layouts, whether from learning, reverse learning, Undo, Forget layout, or a first-seen monitor set, SHALL be reported so it can be stored. Layouts stored earlier SHALL be accepted at start-up and SHALL apply as if learned.

#### Scenario: Learning is reported
- **WHEN** this machine learns its right side toward B
- **THEN** the full set of layouts, including the new side, is reported for storage
