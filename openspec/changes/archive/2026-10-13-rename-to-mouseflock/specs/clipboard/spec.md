## MODIFIED Requirements

### Requirement: Images only to members that understand them
A machine SHALL announce an image copy only to members whose connection agreed protocol version 2 or later. It SHALL announce text copies to every up member, whatever version their connection agreed.

#### Scenario: Member running 0.1.0
- **WHEN** A runs this release, C speaks only protocol version 1 and agreed version 1, and A's user copies an image
- **THEN** C receives no `ClipboardOffer` for it, C's connection stays up, and text copies still reach C
