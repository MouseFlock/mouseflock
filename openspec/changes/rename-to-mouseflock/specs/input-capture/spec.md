## MODIFIED Requirements

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

### Requirement: Injected input is never withheld
Injected input SHALL reach this machine's OS in every capture mode.

#### Scenario: Injection while withholding
- **WHEN** the capture mode is `withhold all` and MouseFlock injects a key press
- **THEN** the focused application receives the key
