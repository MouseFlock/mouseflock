# input-capture Specification

## Purpose

Observes this machine's keyboard and mouse input on Windows, tells physical input from injected input, keeps physical input from the local OS when focus is elsewhere, injects input on a peer's behalf, and reports the monitors and cursor, so that the engine's decisions are carried out exactly on the user's desktop.

## Requirements

### Requirement: Local input is reported
Every key, mouse button, wheel and mouse movement event from this machine's keyboards and mice, and every such event injected by software, SHALL be reported in the order the OS delivered it. Each report SHALL carry a millisecond timestamp from a monotonic clock and the event's origin. Holding a key down SHALL report a press for each repeat the keyboard generates.

#### Scenario: Key press and release
- **WHEN** the user presses and releases a key
- **THEN** a press and then a release of that key are reported, the release's timestamp no earlier than the press's

#### Scenario: Wheel notch
- **WHEN** the user turns the wheel one notch away from them
- **THEN** a wheel event with vertical movement +120 is reported

#### Scenario: Held key repeats
- **WHEN** the user holds a key down past the keyboard's repeat delay
- **THEN** further presses of that key are reported at the repeat rate, followed by one release

### Requirement: Keys are reported by position
A key SHALL be reported as the USB HID keyboard-page usage of its position on the keyboard, whatever the active keyboard layout. Keys that share a meaning but have different positions, such as left and right Ctrl or the main and numeric-keypad Enter, SHALL be reported as different usages.

#### Scenario: Layout does not change the usage
- **WHEN** the active layout is French AZERTY and the user presses the key in the position of the US layout's Q
- **THEN** usage 0x14 is reported, the same as with a US layout

#### Scenario: Left and right Ctrl
- **WHEN** the user presses left Ctrl and then right Ctrl
- **THEN** usage 0xE0 and then usage 0xE4 are reported

### Requirement: Keys outside the keyboard page stay local
A key with no USB HID keyboard-page usage, such as a media, volume or vendor key, SHALL NOT be reported and SHALL always reach this machine's OS, whatever the capture mode.

#### Scenario: Volume key while withholding
- **WHEN** the capture mode withholds all input and the user presses a volume key
- **THEN** the volume changes on this machine and no key is reported

### Requirement: Motion is raw movement with the cursor position
A mouse movement SHALL be reported as the device's raw relative movement, before pointer speed and acceleration, together with the cursor position in virtual-screen pixels. Positions SHALL use physical pixels across all monitors, whatever their scaling.

#### Scenario: Pointer speed does not change reported motion
- **WHEN** the pointer speed is set to its slowest and the mouse reports a movement of 10 counts to the right
- **THEN** the reported motion is 10 to the right and 0 vertically

#### Scenario: Position on a scaled monitor
- **WHEN** a monitor at 150% scaling sits at the virtual-screen origin with a resolution of 2880 by 1800, and the cursor is at its bottom-right pixel
- **THEN** the reported cursor position is (2879, 1799)

### Requirement: Origin of each event
An event SHALL be reported as physical when it comes from a keyboard or mouse attached to this machine, and as injected when software produced it, whether that software is MouseFlock or another program.

#### Scenario: Physical key
- **WHEN** the user presses a key on this machine's keyboard
- **THEN** the key is reported as physical

#### Scenario: DeskHop's own injection
- **WHEN** MouseFlock injects a key press on a peer's behalf
- **THEN** that key press is reported as injected

#### Scenario: Another program's injection
- **WHEN** another program injects a mouse movement
- **THEN** that movement is reported as injected

### Requirement: Capture modes withhold physical input
Physical input SHALL be kept from this machine's OS according to the capture mode. In `pass all`, nothing is withheld. In `withhold all`, physical keys, buttons, wheel and movement are withheld. In `withhold mouse`, physical buttons, wheel and movement are withheld and keys reach the OS. Withheld events SHALL still be reported. While movement is withheld, the cursor SHALL not move.

#### Scenario: Withhold all
- **WHEN** the capture mode is `withhold all` and the user types a key and moves the mouse
- **THEN** no application sees the key, the cursor stays still, and both events are reported

#### Scenario: Withhold mouse
- **WHEN** the capture mode is `withhold mouse` and the user types a key and clicks
- **THEN** the focused application receives the key, the click reaches no application, and both events are reported

#### Scenario: Pass all
- **WHEN** the capture mode is `pass all` and the user moves the mouse
- **THEN** the cursor moves as normal and the movement is reported

### Requirement: Injected input is never withheld
Injected input SHALL reach this machine's OS in every capture mode.

#### Scenario: Injection while withholding
- **WHEN** the capture mode is `withhold all` and MouseFlock injects a key press
- **THEN** the focused application receives the key

### Requirement: Withholding never waits
The decision to pass or withhold an event SHALL depend only on the capture mode and the event's origin and kind. It SHALL NOT wait for anything that consumes reported events. A change of capture mode SHALL apply to every event the OS delivers after the change.

#### Scenario: Consumer stops reading
- **WHEN** nothing reads the reported events for 10 seconds while the capture mode is `pass all`
- **THEN** the user's typing and mouse movement reach the OS without delay throughout

#### Scenario: Mode change applies to the next event
- **WHEN** the capture mode changes from `pass all` to `withhold all` and the user then moves the mouse
- **THEN** the cursor does not move

### Requirement: Injecting input
An input action SHALL be injected into this machine's OS as follows:
- a key by its position, so that this machine's keyboard layout decides the character;
- motion as relative movement, so that this machine's pointer speed and acceleration apply;
- buttons and wheel as given.

#### Scenario: Injected key follows the local layout
- **WHEN** this machine's layout is French AZERTY and a press and release of usage 0x14 are injected
- **THEN** the focused application receives the character "a"

#### Scenario: Injected motion follows the local pointer speed
- **WHEN** a relative movement of 10 to the right is injected
- **THEN** the cursor moves as it would for a 10-count movement of this machine's own mouse at this machine's pointer speed

### Requirement: Cursor warp
The cursor SHALL be movable to a given point in virtual-screen pixels. A warp SHALL NOT be reported as motion.

#### Scenario: Warp to the exit point
- **WHEN** the cursor is warped to (1919, 540)
- **THEN** the cursor is at (1919, 540) and no motion event is reported for the warp

### Requirement: Screen reports
The monitors SHALL be reported once at start and again whenever the set of monitors, their positions or sizes, or their scaling changes. A report SHALL NOT repeat an unchanged screen. Each monitor SHALL be reported with its rectangle in virtual-screen physical pixels and its effective DPI (96 at 100% scaling). Monitors that mirror the same desktop area SHALL be reported as one monitor.

#### Scenario: Monitor connected
- **WHEN** a second monitor is connected and extends the desktop
- **THEN** a new screen with both monitors is reported

#### Scenario: Scaling changed
- **WHEN** the user changes a monitor's scaling from 100% to 150%
- **THEN** a new screen is reported in which that monitor's DPI is 144

#### Scenario: Nothing changed
- **WHEN** the OS signals a display change but no monitor's set membership, rectangle or DPI differs
- **THEN** no screen is reported

#### Scenario: Mirrored displays
- **WHEN** two displays mirror the same desktop area
- **THEN** the screen contains one monitor for that area

### Requirement: Monitor identity
Each monitor SHALL have an identity that stays the same across restarts. When the monitor reports a manufacturer, product and nonzero serial number that no other connected monitor shares, its identity SHALL be built from those, so it survives a change of port or dock. Otherwise its identity SHALL be built from the connection it is attached to.

#### Scenario: Same monitor through another port
- **WHEN** a monitor with a serial number moves from one display port to another
- **THEN** its identity is unchanged

#### Scenario: Two identical monitors without serials
- **WHEN** two monitors of the same model report no serial number
- **THEN** they have different identities, each tied to its connection

#### Scenario: Restart
- **WHEN** the machine restarts with the same monitors on the same ports
- **THEN** every monitor has the same identity as before

### Requirement: Nothing stays withheld after capture ends
When capture stops, or the process running it exits for any reason, no input SHALL remain withheld from this machine's OS.

#### Scenario: Process ends while withholding
- **WHEN** the capture mode is `withhold all` and the process running capture is ended
- **THEN** the user's keyboard and mouse work normally on this machine

### Requirement: Elevated windows
While the agent runs with UI access, input SHALL work with a window that runs elevated on the same desktop as with any other window. Injected keys, buttons, wheel and motion SHALL reach it. Physical input aimed at it SHALL be reported, and passed or withheld by the capture mode.

#### Scenario: Typing into an elevated window
- **WHEN** an elevated Notepad has focus and the agent injects a press and release of usage 0x04
- **THEN** Notepad receives the character "a"

#### Scenario: Moving over an elevated window
- **WHEN** an elevated window has focus and the agent injects a relative movement of 50 to the left
- **THEN** the cursor moves 50 counts' worth to the left

#### Scenario: Physical key while an elevated window has focus
- **WHEN** an elevated window has focus, the capture mode is `withhold all`, and the user presses a key on this machine's keyboard
- **THEN** the key is reported as physical and does not reach the elevated window

#### Scenario: Agent without UI access
- **WHEN** the agent runs without UI access and an elevated window has focus
- **THEN** injected input may not reach that window, and the agent keeps running and carrying out later commands
