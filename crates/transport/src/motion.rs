//! Forwarded motion as running totals (design D3, D4): the sender coalesces
//! movement into `Motion` datagrams and marks, and the receiver turns marks
//! back into movement. Both are pure; the connection task passes the time
//! in.

use std::time::Duration;

use proto::input::Mark;
use tokio::time::Instant;

/// At most one `Motion` datagram per connection this often.
pub const COALESCE: Duration = Duration::from_millis(4);

/// How long after the last datagram, with no movement since, it is sent
/// once more.
pub const REPEAT: Duration = Duration::from_millis(20);

/// True if `a` is newer than `b` within half the sequence space.
fn newer(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

/// Turns marks back into movement.
#[derive(Debug, Clone, Copy, Default)]
pub struct Receiver {
    last: Mark,
}

impl Receiver {
    /// Applies a mark from a `Motion` datagram or an input message; returns
    /// the movement not yet applied, if any.
    pub fn apply(&mut self, mark: Mark) -> Option<(i32, i32)> {
        if !newer(mark.seq, self.last.seq) {
            return None;
        }
        let d = (
            mark.dx.wrapping_sub(self.last.dx),
            mark.dy.wrapping_sub(self.last.dy),
        );
        self.last = mark;
        (d != (0, 0)).then_some(d)
    }
}

/// Coalesces forwarded movement into `Motion` datagrams and marks.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sender {
    /// Totals of all movement added, sent or not.
    dx: i32,
    dy: i32,
    /// The last mark sent or embedded.
    last: Mark,
    /// Movement added since `last`.
    pending: bool,
    /// When the last datagram was sent.
    sent_at: Option<Instant>,
    flush_at: Option<Instant>,
    repeat_at: Option<Instant>,
}

impl Sender {
    /// Adds movement; returns a datagram to send now, or `None` if it waits
    /// for [`Sender::poll`].
    pub fn add(&mut self, dx: i32, dy: i32, now: Instant) -> Option<Mark> {
        self.dx = self.dx.wrapping_add(dx);
        self.dy = self.dy.wrapping_add(dy);
        self.pending = true;
        self.repeat_at = None;
        match self.sent_at {
            Some(at) if now < at + COALESCE => {
                self.flush_at = Some(at + COALESCE);
                None
            }
            _ => Some(self.datagram(now)),
        }
    }

    /// The datagram due at `now`, if any: pending movement at its flush
    /// time, or the repeat of the last datagram.
    pub fn poll(&mut self, now: Instant) -> Option<Mark> {
        if self.flush_at.is_some_and(|at| now >= at) {
            return Some(self.datagram(now));
        }
        if self.repeat_at.is_some_and(|at| now >= at) {
            self.repeat_at = None;
            self.sent_at = Some(now);
            return Some(self.last);
        }
        None
    }

    /// The mark for a key, button or wheel message. Pending movement takes
    /// the next sequence number and needs no datagram of its own.
    pub fn mark(&mut self) -> Mark {
        if self.pending {
            self.advance();
            self.flush_at = None;
        }
        self.last
    }

    /// When [`Sender::poll`] next has something to send.
    pub fn next_deadline(&self) -> Option<Instant> {
        match (self.flush_at, self.repeat_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn datagram(&mut self, now: Instant) -> Mark {
        self.advance();
        self.sent_at = Some(now);
        self.flush_at = None;
        self.repeat_at = Some(now + REPEAT);
        self.last
    }

    fn advance(&mut self) {
        self.last = Mark {
            seq: self.last.seq.wrapping_add(1),
            dx: self.dx,
            dy: self.dy,
        };
        self.pending = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(seq: u32, dx: i32, dy: i32) -> Mark {
        Mark { seq, dx, dy }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn lost_datagram_loses_no_movement() {
        let mut r = Receiver::default();
        assert_eq!(r.apply(mark(1, 5, 0)), Some((5, 0)));
        // Sequence 2 (totals 10) is lost.
        assert_eq!(r.apply(mark(3, 15, 0)), Some((10, 0)));
    }

    #[test]
    fn late_and_duplicate_marks_are_ignored() {
        let mut r = Receiver::default();
        assert_eq!(r.apply(mark(8, 40, 2)), Some((40, 2)));
        assert_eq!(r.apply(mark(7, 35, 2)), None);
        assert_eq!(r.apply(mark(8, 40, 2)), None);
        assert_eq!(r.apply(mark(9, 41, 2)), Some((1, 0)));
    }

    #[test]
    fn zero_movement_is_not_reported() {
        let mut r = Receiver::default();
        assert_eq!(r.apply(mark(1, 0, 0)), None);
        assert_eq!(r.apply(mark(2, 3, 3)), Some((3, 3)));
        assert_eq!(r.apply(mark(3, 3, 3)), None);
    }

    #[test]
    fn totals_wrap() {
        let mut r = Receiver::default();
        r.apply(mark(1, i32::MAX - 2, i32::MIN + 1));
        assert_eq!(r.apply(mark(2, i32::MIN + 2, i32::MAX)), Some((5, -2)));
    }

    #[test]
    fn sequence_numbers_wrap() {
        let mut r = Receiver::default();
        assert_eq!(r.apply(mark(u32::MAX - 1, 1, 0)), None, "older than 0");
        r.apply(mark(1, 1, 0));
        r.apply(mark(0x7fff_ffff, 2, 0));
        r.apply(mark(0xffff_fff0, 3, 0));
        assert_eq!(r.apply(mark(u32::MAX, 4, 0)), Some((1, 0)));
        assert_eq!(r.apply(mark(0, 5, 0)), Some((1, 0)));
        assert_eq!(r.apply(mark(1, 6, 0)), Some((1, 0)));
        assert_eq!(r.apply(mark(u32::MAX, 9, 0)), None);
    }

    #[test]
    fn first_movement_is_sent_at_once() {
        let t = Instant::now();
        let mut s = Sender::default();
        assert_eq!(s.add(3, -1, t), Some(mark(1, 3, -1)));
    }

    #[test]
    fn a_burst_coalesces() {
        let t = Instant::now();
        let mut s = Sender::default();
        let mut sent = Vec::new();
        for i in 0..8u64 {
            let now = t + Duration::from_micros(i * 125);
            sent.extend(s.add(1, 0, now));
            sent.extend(s.poll(now));
        }
        // The rest waits for the flush, 4 ms after the first datagram.
        assert_eq!(s.next_deadline(), Some(t + COALESCE));
        assert_eq!(s.poll(t + ms(3)), None);
        sent.extend(s.poll(t + COALESCE));
        assert_eq!(sent, [mark(1, 1, 0), mark(2, 8, 0)]);
    }

    #[test]
    fn flush_is_exactly_one_interval_after_the_last_datagram() {
        let t = Instant::now();
        let mut s = Sender::default();
        s.add(1, 0, t);
        assert_eq!(s.add(1, 0, t + ms(1)), None);
        assert_eq!(s.poll(t + ms(4) - Duration::from_micros(1)), None);
        assert_eq!(s.poll(t + ms(4)), Some(mark(2, 2, 0)));
        // A movement 4 ms after that goes at once.
        assert_eq!(s.add(1, 0, t + ms(8)), Some(mark(3, 3, 0)));
    }

    #[test]
    fn last_datagram_is_repeated_once() {
        let t = Instant::now();
        let mut s = Sender::default();
        s.add(5, 0, t);
        assert_eq!(s.next_deadline(), Some(t + REPEAT));
        assert_eq!(s.poll(t + ms(19)), None);
        assert_eq!(s.poll(t + REPEAT), Some(mark(1, 5, 0)));
        assert_eq!(s.next_deadline(), None);
        assert_eq!(s.poll(t + ms(100)), None);
    }

    #[test]
    fn movement_cancels_the_repeat() {
        let t = Instant::now();
        let mut s = Sender::default();
        s.add(5, 0, t);
        assert_eq!(s.add(1, 0, t + ms(1)), None);
        assert_eq!(s.mark(), mark(2, 6, 0));
        // The mark carried the movement; no repeat, no flush.
        assert_eq!(s.next_deadline(), None);
    }

    #[test]
    fn mark_takes_over_pending_movement() {
        let t = Instant::now();
        let mut s = Sender::default();
        s.add(1, 0, t);
        assert_eq!(s.add(2, 0, t + ms(1)), None);
        assert_eq!(s.mark(), mark(2, 3, 0));
        // No datagram is sent for it.
        assert_eq!(s.poll(t + COALESCE), None);
    }

    #[test]
    fn mark_with_nothing_pending_repeats_the_last_sequence() {
        let t = Instant::now();
        let mut s = Sender::default();
        assert_eq!(s.mark(), mark(0, 0, 0));
        s.add(4, 4, t);
        assert_eq!(s.mark(), mark(1, 4, 4));
        assert_eq!(s.mark(), mark(1, 4, 4));
    }

    /// xorshift64, for a deterministic lossy run.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    #[test]
    fn sender_and_receiver_converge_through_loss() {
        let mut rng = Rng(0x853c_49e6_748f_ea9b);
        let mut s = Sender::default();
        let mut r = Receiver::default();
        let (mut sent, mut got) = ((0i64, 0i64), (0i64, 0i64));
        let mut now = Instant::now();
        let apply = |r: &mut Receiver, m: Mark, got: &mut (i64, i64)| {
            if let Some((dx, dy)) = r.apply(m) {
                got.0 += i64::from(dx);
                got.1 += i64::from(dy);
            }
        };
        for _ in 0..20_000 {
            now += Duration::from_micros(rng.next() % 3_000);
            if rng.next().is_multiple_of(10) {
                // A click: its mark travels on the stream and always arrives.
                apply(&mut r, s.mark(), &mut got);
                assert_eq!(got, sent);
            } else {
                let dx = (rng.next() % 21) as i32 - 10;
                let dy = (rng.next() % 21) as i32 - 10;
                sent.0 += i64::from(dx);
                sent.1 += i64::from(dy);
                // A third of datagrams are lost.
                if let Some(m) = s.add(dx, dy, now)
                    && !rng.next().is_multiple_of(3)
                {
                    apply(&mut r, m, &mut got);
                }
            }
            if let Some(m) = s.poll(now)
                && !rng.next().is_multiple_of(3)
            {
                apply(&mut r, m, &mut got);
            }
        }
    }
}
