# input-focus Specification

## Purpose

Decides which machine receives a user's keyboard and mouse input, when the cursor crosses from one machine to another, how the person at a controlled machine takes it back, and what is released whenever focus moves, so that input never lands on the wrong machine or leaves a key stuck down.

## Requirements

### Requirement: Focus follows the cursor
For the physical input of each machine, exactly one machine SHALL have focus: the machine the cursor is on. While this machine's own input has focus locally, the input SHALL reach this machine's OS unchanged. While it has focus on a peer, this machine's physical key, button, wheel, and motion input SHALL be withheld from its own OS and forwarded to that peer, and this machine's cursor SHALL stay where it left.

#### Scenario: Input stays local
- **WHEN** this machine has focus locally and the user types a key
- **THEN** the key reaches this machine's OS and nothing is sent to any peer

#### Scenario: Input goes to the focused peer
- **WHEN** focus is on peer B and the user types a key on this machine
- **THEN** the key is withheld from this machine's OS and forwarded to B

### Requirement: Physical and injected input are distinguished
Every local input event SHALL be known to be either physical (from a device on this machine) or injected (produced by MouseFlock on behalf of a peer, or by other software). Only physical input SHALL count as the local user acting: injected input SHALL never start a takeover or be forwarded to a peer.

#### Scenario: Injected motion does not take over
- **WHEN** peer A controls this machine and MouseFlock injects A's motion here
- **THEN** that motion does not count as local activity and does not start a takeover

### Requirement: Forwarded motion is raw movement
Motion forwarded to a peer SHALL be the raw device movement, before this machine's pointer speed or acceleration. The peer SHALL apply its own pointer speed when it moves the cursor.

#### Scenario: Different pointer speeds
- **WHEN** this machine has a slow pointer setting, peer B has a fast one, and focus is on B
- **THEN** the raw movement is forwarded unchanged and the cursor on B moves at B's setting

### Requirement: Crossing a mapped edge
The cursor SHALL cross to a peer when all of these hold: it is on an outer side of a monitor (a side with no other monitor beyond it); movement pushes outward through that side; the layout maps that monitor side to the peer; the peer is connected; the cursor is outside the corner zones; and no mouse button is held. With the default edge sensitivity, the crossing SHALL happen on the first outward push.

#### Scenario: Instant crossing
- **WHEN** the right side of this machine's monitor maps to connected peer B, the cursor is mid-way down that side, no button is held, and the user moves the mouse right
- **THEN** focus moves to B on that first push

#### Scenario: Inner side is not an edge
- **WHEN** a second monitor sits directly to the right of the cursor's monitor and the cursor moves right across the boundary
- **THEN** the cursor moves onto the second monitor and focus stays local

### Requirement: Edges that act as walls
An outer monitor side SHALL stop the cursor like a normal screen edge, with no crossing, while the layout maps it to no peer and layout learning does not place a peer there (see the `layout` capability), while the mapped peer is not connected, or while any mouse button is held.

#### Scenario: Dragging at the edge
- **WHEN** the user holds the left button to drag a window and pushes against a side mapped to connected peer B
- **THEN** focus stays local and the drag continues on this machine

#### Scenario: Peer not connected
- **WHEN** a side maps to peer B and B is not connected
- **THEN** pushing against that side does not change focus

#### Scenario: Unmapped side with nothing to learn
- **WHEN** a side maps to no peer and no connected peer is unplaced on this machine
- **THEN** pushing against that side does not change focus

### Requirement: Corner zones never cross
Within 2 mm of either end of a monitor side, measured with that monitor's DPI, pushing outward SHALL never cross, whatever the edge sensitivity.

#### Scenario: Closing a maximized window
- **WHEN** the right side maps to connected peer B and the user flings the cursor into the top-right corner of the monitor
- **THEN** the cursor stops in the corner and focus stays local

#### Scenario: Corner size follows DPI
- **WHEN** a monitor runs at 192 DPI
- **THEN** its corner zones are about 15 pixels long, twice the length on a 96 DPI monitor

### Requirement: Edge sensitivity setting
The engine SHALL support two edge sensitivities. `instant` (the default) SHALL cross a mapped edge on the first outward push. `push` SHALL cross only after at least 20 device units of outward movement against the edge, accumulated with no pause longer than 250 ms; moving away from the edge or pausing SHALL reset the count.

#### Scenario: Push sensitivity needs a deliberate push
- **WHEN** the sensitivity is `push` and the user moves 12 units outward against a mapped edge, then stops
- **THEN** focus stays local

#### Scenario: Push sensitivity crosses on a sustained push
- **WHEN** the sensitivity is `push` and the user moves 25 units outward against a mapped edge without pausing
- **THEN** focus moves to the mapped peer

#### Scenario: Pause resets the push
- **WHEN** the sensitivity is `push`, the user moves 15 units outward, pauses 300 ms, then moves 10 more
- **THEN** focus stays local

### Requirement: Entry position
When focus moves to a machine through an edge, the cursor SHALL appear on that machine's side opposite the exit side, at the same relative position along the side as where it left, measured across the whole extent of each machine's screens on that axis. If that point is not on a monitor, the cursor SHALL appear at the nearest point on a monitor along that side.

#### Scenario: Same relative height
- **WHEN** the cursor leaves this machine's right side 25% of the way down its screen extent
- **THEN** it appears on the peer's left side 25% of the way down the peer's screen extent

### Requirement: Crossing out of a controlled machine
A machine controlled by a peer SHALL apply the crossing rules to its own edges, using its own layout and settings, and SHALL tell the controlling machine which peer the cursor left towards and where. If that peer is the controlling machine, focus SHALL return to it at the entry position. If it is a third peer, the controlling machine SHALL move focus there if it is connected to it; otherwise the edge SHALL act as a wall.

#### Scenario: Back to the controlling machine
- **WHEN** A controls B, B's left side maps to A, and the forwarded movement pushes the cursor out of B's left side 60% of the way down
- **THEN** focus returns to A, and A's cursor appears on A's right side 60% of the way down

#### Scenario: On to a third machine
- **WHEN** A controls B, B's right side maps to C, A is connected to C, and the cursor leaves B's right side
- **THEN** focus moves from B to C, and A forwards its input to C

### Requirement: One controller at a time
A machine SHALL accept focus from at most one peer at a time. While one peer controls it, a crossing from any other peer SHALL be refused, and the refused peer SHALL keep focus locally with its cursor at the point where it tried to leave, as if the edge were a wall.

#### Scenario: Second controller refused
- **WHEN** A controls B and the cursor on C crosses an edge mapped to B
- **THEN** B refuses, C keeps focus locally, and A keeps control of B

### Requirement: Focus changes release held input
Whenever focus moves away from a machine, every key and mouse button held down on that machine as a result of the input that had focus SHALL be released there. A key or button release forwarded to a machine that never received the matching press SHALL be dropped.

#### Scenario: Crossing with Shift held
- **WHEN** the user holds Shift and the cursor crosses from this machine to peer B
- **THEN** Shift is released on this machine, and B does not see Shift as held

#### Scenario: Release after the crossing
- **WHEN** the user presses Ctrl on this machine, the cursor crosses to B, and the user then lets go of Ctrl
- **THEN** B does not receive a Ctrl release, because it never received the press

#### Scenario: Leaving a controlled machine with a key held
- **WHEN** A controls B, the user holds Alt, and focus returns to A
- **THEN** B releases Alt

### Requirement: Taking control back locally
While a peer controls this machine, local physical input SHALL take focus back as follows. A local key press SHALL take focus back immediately and SHALL reach this machine's OS. Local mouse movement SHALL be grouped into bursts, a new burst starting after 150 ms without local movement. The first burst SHALL be withheld from this machine's OS and SHALL NOT take focus back. A second burst starting within 2 s of the first, or a single burst lasting 2 s, SHALL take focus back. If 2 s pass after a burst without another, local movement SHALL be treated as a first burst again. While control is contested, the controlling peer's input SHALL continue to be applied.

#### Scenario: Accidental nudge
- **WHEN** A controls this machine and its mouse is nudged once briefly
- **THEN** the nudge does not move this machine's cursor and A keeps control

#### Scenario: Second movement takes over
- **WHEN** A controls this machine, its mouse is nudged, and 1 s later it is moved again
- **THEN** this machine takes focus back

#### Scenario: Sustained movement takes over
- **WHEN** A controls this machine and its mouse moves continuously for 2 s
- **THEN** this machine takes focus back

#### Scenario: Key press takes over
- **WHEN** A controls this machine and someone types a key on this machine's keyboard
- **THEN** this machine takes focus back at once and the key is typed here

### Requirement: Losing control returns the cursor
When a controlled machine takes focus back, or the connection to a peer is lost while focus is on it, the machine whose input had focus there SHALL get focus back locally with its cursor at the point where it left, and the focus change SHALL release held input as for any other focus change.

#### Scenario: Controlled machine takes over
- **WHEN** A's cursor left A's right side at a point P to control B, and B takes focus back
- **THEN** A has focus locally and its cursor is at P

#### Scenario: Peer lost while focused
- **WHEN** focus is on peer B, the user holds Ctrl, and the connection to B is lost
- **THEN** this machine has focus locally, its cursor is where it left, and Ctrl is not held on this machine

#### Scenario: Controller lost while controlled
- **WHEN** A controls this machine, a key injected for A is held down here, and the connection to A is lost
- **THEN** this machine releases that key

### Requirement: Secure attention request
While focus is on a peer, pressing Ctrl+Alt+End on this machine SHALL be withheld from both machines as a key press and SHALL instead ask the focused peer to show its secure attention screen (the Ctrl-Alt-Del screen). While focus is local, Ctrl+Alt+End SHALL reach this machine's OS unchanged.

#### Scenario: Ctrl+Alt+End while controlling a peer
- **WHEN** focus is on peer B and the user presses Ctrl+Alt+End
- **THEN** B is asked to show its secure attention screen and B does not receive End

#### Scenario: Ctrl+Alt+End with local focus
- **WHEN** focus is local and the user presses Ctrl+Alt+End
- **THEN** the keys reach this machine's OS and no peer is asked for anything

### Requirement: Losing local input
When this machine's input agent ends, focus that depends on it SHALL be returned and held input released. While this machine's input had focus on a peer, the peer SHALL release everything it held for this machine and take focus back. While a peer controlled this machine, the peer SHALL get focus back as if this machine's user had taken control. Focus here SHALL become local.

#### Scenario: Agent ends while forwarding
- **WHEN** focus is on peer B, the user holds Ctrl, and this machine's agent ends
- **THEN** B releases Ctrl and takes focus back, and this machine's focus is local

#### Scenario: Agent ends while controlled
- **WHEN** A controls this machine and this machine's agent ends
- **THEN** A gets focus back locally with its cursor where it left, and this machine's focus is local

#### Scenario: Agent ends with local focus
- **WHEN** focus is local and this machine's agent ends
- **THEN** nothing is sent to any peer

### Requirement: Bringing input home from the keyboard
While this machine's input has focus on a peer, pressing Scroll Lock twice within 500 ms on this machine SHALL return focus to this machine locally, with the cursor at the point where it left, and SHALL release every key and mouse button held for that peer. This SHALL take effect from this machine alone, without waiting for the peer. The peer SHALL be told focus was withdrawn, so it releases what it held and takes focus back, but the return home SHALL NOT depend on that message being answered.

#### Scenario: Peer is unresponsive
- **WHEN** this machine's input is forwarded to peer B, B is injecting nothing back and its cursor is not moving, and the user presses Scroll Lock twice within 500 ms
- **THEN** this machine has focus locally with its cursor where it left, and its physical keyboard and mouse reach this machine again

#### Scenario: Held keys released on return
- **WHEN** the user holds Ctrl, the cursor is forwarding to B, and the user presses Scroll Lock twice within 500 ms
- **THEN** focus is local on this machine and B is told to release what it held for this machine, including Ctrl

#### Scenario: Too slow is not a trigger
- **WHEN** this machine is forwarding to B and the user presses Scroll Lock, then presses it again 800 ms later
- **THEN** focus stays on B, and neither press reaches this machine's OS or B

### Requirement: Scroll Lock is withheld from the peer while forwarding
While this machine's input has focus on a peer, Scroll Lock presses and releases SHALL be withheld from this machine's OS and SHALL NOT be forwarded to the peer, so Scroll Lock never toggles on the peer. While focus is local, Scroll Lock SHALL reach this machine's OS unchanged.

#### Scenario: Scroll Lock not sent to the peer
- **WHEN** this machine is forwarding to B and the user presses Scroll Lock once
- **THEN** B does not receive a Scroll Lock press, and this machine's OS does not either

#### Scenario: Scroll Lock works locally
- **WHEN** focus is local and the user presses Scroll Lock
- **THEN** the key reaches this machine's OS and nothing is sent to any peer
