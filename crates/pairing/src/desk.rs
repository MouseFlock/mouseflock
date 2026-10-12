//! The membership view, this machine's signing side of it, and the trust store
//! file.

use std::collections::BTreeMap;

use model::PeerId;
use proto::pairing::{MemberRecord, RecordsDone};

use crate::FileError;
use crate::identity::Identity;
use crate::record::{Change, MachineName, SignedRecord};

/// Most subjects a view holds records for.
pub const MAX_SUBJECTS: usize = 256;

/// Seconds since the Unix epoch, from the caller's wall clock. Used only to
/// order membership records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct UnixTime(pub u64);

const MAGIC: &[u8; 4] = b"MFTS";
const VERSION: u8 = 1;

/// One machine's view of its desk: the winning record for each subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Desk {
    me: PeerId,
    records: BTreeMap<PeerId, SignedRecord>,
}

/// What applying records changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Machines that became members.
    pub added: Vec<PeerId>,
    /// Machines that stopped being members.
    pub removed: Vec<PeerId>,
    /// A removal of this machine won: the view now holds only this machine.
    pub forgotten: bool,
    /// Records not accepted: a bad signature, an unknown signer, or no room.
    pub dropped: usize,
    /// Records that won and were stored, in the order they were accepted.
    pub accepted: Vec<SignedRecord>,
}

enum Outcome {
    Accepted,
    /// Valid, but its signer is not a member yet.
    Unknown(SignedRecord),
    Rejected,
}

impl Desk {
    /// A view holding only `me`.
    pub fn new(me: PeerId) -> Self {
        Self {
            me,
            records: BTreeMap::new(),
        }
    }

    pub fn me(&self) -> PeerId {
        self.me
    }

    /// This machine, or a subject whose winning record is an addition.
    pub fn is_member(&self, peer: &PeerId) -> bool {
        *peer == self.me || self.records.get(peer).is_some_and(|r| !r.is_removal())
    }

    /// Every other member and its name.
    pub fn members(&self) -> impl Iterator<Item = (PeerId, &MachineName)> {
        self.records
            .values()
            .filter(|r| r.subject != self.me)
            .filter_map(|r| match &r.change {
                Change::Add(name) => Some((r.subject, name)),
                Change::Remove => None,
            })
    }

    /// Every winning record, in subject order.
    pub fn records(&self) -> impl Iterator<Item = &SignedRecord> {
        self.records.values()
    }

    pub fn record(&self, subject: &PeerId) -> Option<&SignedRecord> {
        self.records.get(subject)
    }

    /// The epoch for a new record about `subject`: the current Unix time, or
    /// one above the highest epoch held for it if that is later, and at least
    /// 1. A later action wins on every member, whichever member signed it.
    fn next_epoch(&self, subject: &PeerId, now: UnixTime) -> u64 {
        let above_held = self
            .records
            .get(subject)
            .map_or(1, |r| r.epoch.saturating_add(1));
        above_held.max(now.0)
    }

    /// Applies records received in one exchange, retrying those whose signer
    /// is not yet a member until no more can be accepted.
    pub fn apply(&mut self, records: impl IntoIterator<Item = SignedRecord>) -> Applied {
        let mut applied = Applied::default();
        let mut pending: Vec<SignedRecord> = records.into_iter().collect();
        loop {
            let before = pending.len();
            let mut waiting = Vec::new();
            for record in pending {
                match self.accept(record, &mut applied) {
                    Outcome::Accepted => {}
                    Outcome::Unknown(record) => waiting.push(record),
                    Outcome::Rejected => applied.dropped += 1,
                }
            }
            pending = waiting;
            if pending.len() == before || pending.is_empty() {
                break;
            }
        }
        applied.dropped += pending.len();
        applied
    }

    fn accept(&mut self, record: SignedRecord, applied: &mut Applied) -> Outcome {
        if !record.is_valid() {
            return Outcome::Rejected;
        }
        if !self.is_member(&record.signer) {
            return Outcome::Unknown(record);
        }
        self.insert(record, applied)
    }

    /// Keeps `record` if it wins over what is held for its subject.
    fn insert(&mut self, record: SignedRecord, applied: &mut Applied) -> Outcome {
        let subject = record.subject;
        match self.records.get(&subject) {
            Some(held) if held.rank() >= record.rank() => return Outcome::Accepted,
            None if self.records.len() >= MAX_SUBJECTS => return Outcome::Rejected,
            _ => {}
        }
        let was_member = self.is_member(&subject);
        let removes_me = subject == self.me && record.is_removal();
        applied.accepted.push(record.clone());
        self.records.insert(subject, record);
        if removes_me {
            self.forget();
            applied.forgotten = true;
            return Outcome::Accepted;
        }
        match (was_member, self.is_member(&subject)) {
            (false, true) => applied.added.push(subject),
            (true, false) => applied.removed.push(subject),
            _ => {}
        }
        Outcome::Accepted
    }

    /// Drops every record: this machine is alone again.
    fn forget(&mut self) {
        self.records.clear();
    }

    /// The trust store file: `MFTS`, version 1, a u16 count, then each record
    /// as a u16 length and its `MemberRecord` encoding.
    pub fn to_file(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        let count = u16::try_from(self.records.len()).unwrap_or(u16::MAX);
        out.extend_from_slice(&count.to_le_bytes());
        for record in self.records.values() {
            let payload = record.wire().payload();
            let len = u16::try_from(payload.len()).unwrap_or(u16::MAX);
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&payload);
        }
        out
    }

    /// Reads a trust store file for the machine `me`. Every record's signature
    /// is checked; who signed it is not, since the file is this machine's own.
    pub fn from_file(me: PeerId, bytes: &[u8]) -> Result<Self, FileError> {
        let (magic, rest) = bytes.split_first_chunk::<4>().ok_or(FileError::Length)?;
        if magic != MAGIC {
            return Err(FileError::Magic);
        }
        let (&version, rest) = rest.split_first().ok_or(FileError::Length)?;
        if version != VERSION {
            return Err(FileError::Version);
        }
        let (count, mut rest) = rest.split_first_chunk::<2>().ok_or(FileError::Length)?;
        let count = usize::from(u16::from_le_bytes(*count));
        if count > MAX_SUBJECTS {
            return Err(FileError::Length);
        }
        let mut desk = Self::new(me);
        for _ in 0..count {
            let (len, after) = rest.split_first_chunk::<2>().ok_or(FileError::Length)?;
            let (payload, after) = after
                .split_at_checked(usize::from(u16::from_le_bytes(*len)))
                .ok_or(FileError::Length)?;
            let wire = MemberRecord::decode(payload).map_err(|_| FileError::Record)?;
            let record = SignedRecord::from_wire(&wire);
            if !record.is_valid() || desk.records.contains_key(&record.subject) {
                return Err(FileError::Record);
            }
            desk.records.insert(record.subject, record);
            rest = after;
        }
        if !rest.is_empty() {
            return Err(FileError::Length);
        }
        Ok(desk)
    }
}

/// Why this machine could not sign an addition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddError {
    /// A machine cannot add itself.
    Myself,
    /// The view already holds records for [`MAX_SUBJECTS`] machines.
    Full,
}

/// This machine: its identity, its name, and its view of the desk.
#[derive(Debug)]
pub struct Machine {
    identity: Identity,
    name: MachineName,
    desk: Desk,
}

impl Machine {
    /// A machine alone in its desk.
    pub fn new(identity: Identity, name: MachineName) -> Self {
        let desk = Desk::new(identity.peer_id());
        Self {
            identity,
            name,
            desk,
        }
    }

    pub fn peer_id(&self) -> PeerId {
        self.identity.peer_id()
    }

    pub fn name(&self) -> &MachineName {
        &self.name
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub fn desk(&self) -> &Desk {
        &self.desk
    }

    /// Replaces the view with a trust store file's.
    pub fn load_trust_store(&mut self, bytes: &[u8]) -> Result<(), FileError> {
        self.desk = Desk::from_file(self.peer_id(), bytes)?;
        Ok(())
    }

    pub fn trust_store(&self) -> Vec<u8> {
        self.desk.to_file()
    }

    /// Signs an addition of `peer` at `now` and adds it to this machine's view.
    pub fn add(
        &mut self,
        peer: PeerId,
        name: MachineName,
        now: UnixTime,
    ) -> Result<SignedRecord, AddError> {
        if peer == self.peer_id() {
            return Err(AddError::Myself);
        }
        if self.desk.record(&peer).is_none() && self.desk.records.len() >= MAX_SUBJECTS {
            return Err(AddError::Full);
        }
        let epoch = self.desk.next_epoch(&peer, now);
        let record = SignedRecord::sign(&self.identity, peer, epoch, Change::Add(name));
        self.desk.insert(record.clone(), &mut Applied::default());
        Ok(record)
    }

    /// Signs a removal of `peer` at `now`, which may be this machine, and
    /// applies it.
    /// Removing this machine forgets the desk; send the returned record to the
    /// members first.
    pub fn remove(&mut self, peer: PeerId, now: UnixTime) -> SignedRecord {
        let epoch = self.desk.next_epoch(&peer, now);
        let record = SignedRecord::sign(&self.identity, peer, epoch, Change::Remove);
        self.desk.insert(record.clone(), &mut Applied::default());
        record
    }

    /// Applies records received from a member.
    pub fn apply(&mut self, records: impl IntoIterator<Item = SignedRecord>) -> Applied {
        self.desk.apply(records)
    }

    /// The frames for this side of a record exchange: every winning record,
    /// then `RecordsDone`.
    pub fn record_frames(&self) -> Vec<Vec<u8>> {
        let mut frames: Vec<Vec<u8>> = self
            .desk
            .records()
            .map(|r| {
                let mut out = Vec::new();
                // A record is at most 201 bytes, far under the frame limit.
                let _ = r.wire().encode_frame(&mut out);
                out
            })
            .collect();
        let mut done = Vec::new();
        let _ = RecordsDone.encode_frame(&mut done);
        frames.push(done);
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_rng;

    /// A clock at 0: epochs then just count up from what is held.
    const T: UnixTime = UnixTime(0);

    fn machine(seed: u64, name: &str) -> Machine {
        Machine::new(
            Identity::generate(&mut test_rng(seed)),
            MachineName::new(name).unwrap(),
        )
    }

    fn name(s: &str) -> MachineName {
        MachineName::new(s).unwrap()
    }

    /// A record about `subject` signed by `signer` at `epoch`.
    fn record(signer: &Machine, subject: PeerId, epoch: u64, change: Change) -> SignedRecord {
        SignedRecord::sign(signer.identity(), subject, epoch, change)
    }

    /// A and B already trust each other; C is a third machine.
    fn desk_of_two() -> (Machine, Machine, Machine) {
        let (mut a, mut b, c) = (machine(1, "A"), machine(2, "B"), machine(3, "C"));
        a.add(b.peer_id(), name("B"), T).unwrap();
        b.add(a.peer_id(), name("A"), T).unwrap();
        (a, b, c)
    }

    #[test]
    fn later_removal_wins() {
        let (mut a, b, c) = desk_of_two();
        a.add(c.peer_id(), name("C"), T).unwrap();
        assert!(a.desk().is_member(&c.peer_id()));
        let applied = a.apply([record(&b, c.peer_id(), 2, Change::Remove)]);
        assert_eq!(applied.removed, [c.peer_id()]);
        assert!(!a.desk().is_member(&c.peer_id()));
    }

    #[test]
    fn stale_addition_loses() {
        let (mut a, b, c) = desk_of_two();
        a.apply([record(&b, c.peer_id(), 2, Change::Remove)]);
        let applied = a.apply([record(&b, c.peer_id(), 1, Change::Add(name("C")))]);
        assert_eq!(applied, Applied::default());
        assert!(!a.desk().is_member(&c.peer_id()));
    }

    #[test]
    fn removal_wins_a_tie() {
        let (_, b, c) = desk_of_two();
        for order in [false, true] {
            let mut a = machine(1, "A");
            a.add(b.peer_id(), name("B"), T).unwrap();
            let add = record(&b, c.peer_id(), 3, Change::Add(name("C")));
            let remove = record(&b, c.peer_id(), 3, Change::Remove);
            let batch = if order { [add, remove] } else { [remove, add] };
            a.apply(batch);
            assert!(!a.desk().is_member(&c.peer_id()));
        }
    }

    #[test]
    fn re_pairing_a_removed_machine() {
        let (mut a, _, c) = desk_of_two();
        a.add(c.peer_id(), name("C"), T).unwrap();
        let removal = a.remove(c.peer_id(), T);
        assert_eq!(removal.epoch, 2);
        let again = a.add(c.peer_id(), name("C"), T).unwrap();
        assert_eq!(again.epoch, 3);
        assert!(a.desk().is_member(&c.peer_id()));
    }

    #[test]
    fn epoch_is_the_current_time() {
        let (mut a, _, c) = desk_of_two();
        let record = a
            .add(c.peer_id(), name("C"), UnixTime(1_800_000_000))
            .unwrap();
        assert_eq!(record.epoch, 1_800_000_000);
    }

    #[test]
    fn epoch_stays_above_what_is_held_when_the_clock_is_behind() {
        let (mut a, b, c) = desk_of_two();
        a.apply([record(
            &b,
            c.peer_id(),
            2_000_000_000,
            Change::Add(name("C")),
        )]);
        let removal = a.remove(c.peer_id(), UnixTime(1_800_000_000));
        assert_eq!(removal.epoch, 2_000_000_001);
    }

    #[test]
    fn re_pairing_through_a_member_that_missed_the_removal() {
        let (mut a, mut b, c) = desk_of_two();
        b.add(c.peer_id(), name("C"), UnixTime(1_000)).unwrap();
        b.remove(c.peer_id(), UnixTime(2_000));
        // A never heard of C and adds it later.
        a.add(c.peer_id(), name("C"), UnixTime(3_000)).unwrap();
        let from_a: Vec<_> = a.desk().records().cloned().collect();
        let from_b: Vec<_> = b.desk().records().cloned().collect();
        a.apply(from_b);
        b.apply(from_a);
        assert!(a.desk().is_member(&c.peer_id()));
        assert!(b.desk().is_member(&c.peer_id()));
    }

    #[test]
    fn subject_257_is_refused() {
        let mut a = machine(1, "A");
        for seed in 0..MAX_SUBJECTS as u64 {
            let peer = Identity::generate(&mut test_rng(1000 + seed)).peer_id();
            a.add(peer, name("x"), T).unwrap();
        }
        let one_more = Identity::generate(&mut test_rng(5000)).peer_id();
        assert_eq!(a.add(one_more, name("x"), T), Err(AddError::Full));
        let b = a.desk().members().next().unwrap().0;
        let signer = machine(1, "A");
        let applied = a.apply([record(&signer, one_more, 1, Change::Add(name("x")))]);
        assert_eq!(applied.dropped, 1);
        assert!(!a.desk().is_member(&one_more));
        // Records for subjects already held still apply.
        assert!(a.add(b, name("renamed"), T).is_ok());
    }

    #[test]
    fn adding_myself_is_refused() {
        let mut a = machine(1, "A");
        assert_eq!(a.add(a.peer_id(), name("A"), T), Err(AddError::Myself));
    }

    #[test]
    fn accepted_records_are_listed_in_order() {
        let (mut a, b, c) = desk_of_two();
        let d = machine(4, "D");
        let stranger = machine(9, "S");
        let stale = record(&b, c.peer_id(), 1, Change::Add(name("C")));
        a.apply([record(&b, c.peer_id(), 5, Change::Add(name("C")))]);
        // E by D arrives before D is known, so it is accepted second.
        let e = machine(5, "E");
        let e_by_d = record(&d, e.peer_id(), 1, Change::Add(name("E")));
        let d_by_b = record(&b, d.peer_id(), 1, Change::Add(name("D")));
        let by_stranger = record(&stranger, machine(6, "F").peer_id(), 1, Change::Remove);
        let applied = a.apply([e_by_d.clone(), stale, d_by_b.clone(), by_stranger]);
        assert_eq!(applied.accepted, [d_by_b, e_by_d]);
        assert_eq!(applied.dropped, 1);
    }

    #[test]
    fn single_record_lists_what_it_stored() {
        let (mut a, b, c) = desk_of_two();
        let add = record(&b, c.peer_id(), 1, Change::Add(name("C")));
        assert_eq!(a.apply([add.clone()]).accepted, std::slice::from_ref(&add));
        assert!(a.apply([add]).accepted.is_empty());
    }

    #[test]
    fn record_from_a_stranger() {
        let (mut a, _, c) = desk_of_two();
        let stranger = machine(9, "S");
        let applied = a.apply([record(&stranger, c.peer_id(), 1, Change::Add(name("C")))]);
        assert_eq!(applied.dropped, 1);
        assert!(!a.desk().is_member(&c.peer_id()));
    }

    #[test]
    fn records_out_of_dependency_order() {
        let (mut a, b, _) = desk_of_two();
        let d = machine(4, "D");
        let e = machine(5, "E");
        let e_by_d = record(&d, e.peer_id(), 1, Change::Add(name("E")));
        let d_by_b = record(&b, d.peer_id(), 1, Change::Add(name("D")));
        let applied = a.apply([e_by_d, d_by_b]);
        assert_eq!(applied.added, [d.peer_id(), e.peer_id()]);
        assert_eq!(applied.dropped, 0);
    }

    #[test]
    fn record_kept_after_its_signer_is_removed() {
        let (mut a, b, c) = desk_of_two();
        a.apply([record(&b, c.peer_id(), 1, Change::Add(name("C")))]);
        a.remove(b.peer_id(), T);
        assert!(a.desk().is_member(&c.peer_id()));
        assert_eq!(a.desk().record(&c.peer_id()).unwrap().signer, b.peer_id());
    }

    #[test]
    fn removing_myself_forgets_the_desk() {
        let (mut a, b, _) = desk_of_two();
        let removal = a.remove(a.peer_id(), T);
        assert!(removal.is_valid());
        assert!(!a.desk().is_member(&b.peer_id()));
        assert_eq!(a.desk().records().count(), 0);
    }

    #[test]
    fn round_trip() {
        let (mut a, b, c) = desk_of_two();
        a.add(c.peer_id(), name("C"), T).unwrap();
        a.remove(c.peer_id(), T);
        a.apply([record(
            &b,
            machine(7, "G").peer_id(),
            4,
            Change::Add(name("Gaming PC")),
        )]);
        let file = a.trust_store();
        let read = Desk::from_file(a.peer_id(), &file).unwrap();
        assert_eq!(&read, a.desk());
        assert_eq!(read.record(&c.peer_id()).unwrap().epoch, 2);
    }

    #[test]
    fn corrupt_trust_store() {
        let (mut a, _, c) = desk_of_two();
        a.add(c.peer_id(), name("C"), T).unwrap();
        let file = a.trust_store();
        let me = a.peer_id();
        for len in 0..file.len() {
            assert!(
                Desk::from_file(me, &file[..len]).is_err(),
                "{len} bytes accepted"
            );
        }
        let mut long = file.clone();
        long.push(0);
        assert_eq!(Desk::from_file(me, &long), Err(FileError::Length));
        let mut bad_signature = file.clone();
        *bad_signature.last_mut().unwrap() ^= 1;
        assert_eq!(Desk::from_file(me, &bad_signature), Err(FileError::Record));
        let mut magic = file.clone();
        magic[0] = b'X';
        assert_eq!(Desk::from_file(me, &magic), Err(FileError::Magic));
        let mut version = file;
        version[4] = 2;
        assert_eq!(Desk::from_file(me, &version), Err(FileError::Version));
    }

    #[test]
    fn empty_trust_store() {
        let a = machine(1, "A");
        let read = Desk::from_file(a.peer_id(), &a.trust_store()).unwrap();
        assert_eq!(read.records().count(), 0);
        assert!(read.is_member(&a.peer_id()));
    }
}
