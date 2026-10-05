//! Aligning an agent's clock with the service's (agent-link, "Clock
//! alignment").
//!
//! The service sends `Ping(t0)`, the agent answers `Pong(t0, a)` at once, and
//! the service receives it at `t1`. The agent read its clock at about the
//! midpoint, so `offset = a − (t0 + t1) / 2`. The exchange with the shortest
//! round trip gives the tightest bound, so it is the one kept.

use model::Millis;

/// The best offset estimate so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClockSync {
    /// `(round trip, offset)` of the shortest exchange seen.
    best: Option<(u64, i128)>,
}

impl ClockSync {
    /// Records one exchange: `Ping` sent at service time `sent`, carrying
    /// agent time `agent` back, received at service time `received`.
    pub fn observe(&mut self, sent: Millis, agent: Millis, received: Millis) {
        let Some(rtt) = received.0.checked_sub(sent.0) else {
            return;
        };
        if self.best.is_some_and(|(best, _)| best <= rtt) {
            return;
        }
        let midpoint = i128::from(sent.0).saturating_add(i128::from(received.0)) / 2;
        self.best = Some((rtt, i128::from(agent.0).saturating_sub(midpoint)));
    }

    /// True once at least one exchange has been seen.
    pub const fn ready(&self) -> bool {
        self.best.is_some()
    }

    /// Agent clock minus service clock, in milliseconds.
    pub fn offset(&self) -> Option<i128> {
        self.best.map(|(_, offset)| offset)
    }

    /// `at` on the agent's clock, on the service's clock: clamped at 0.
    pub fn to_service(&self, at: Millis) -> Option<Millis> {
        let offset = self.offset()?;
        let t = i128::from(at.0)
            .saturating_sub(offset)
            .clamp(0, i128::from(u64::MAX));
        Some(Millis(u64::try_from(t).unwrap_or(0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_scenario() {
        let mut c = ClockSync::default();
        assert!(!c.ready());
        assert_eq!(c.to_service(Millis(5)), None);
        c.observe(Millis(1_000), Millis(51_002), Millis(1_004));
        assert_eq!(c.offset(), Some(50_000));
        assert_eq!(c.to_service(Millis(51_500)), Some(Millis(1_500)));
    }

    #[test]
    fn shorter_round_trip_wins() {
        let mut c = ClockSync::default();
        c.observe(Millis(0), Millis(500), Millis(10));
        assert_eq!(c.offset(), Some(495));
        c.observe(Millis(100), Millis(601), Millis(102));
        assert_eq!(c.offset(), Some(500));
        // A longer exchange does not replace it.
        c.observe(Millis(200), Millis(900), Millis(230));
        assert_eq!(c.offset(), Some(500));
    }

    #[test]
    fn conversion_clamps_at_zero() {
        let mut c = ClockSync::default();
        c.observe(Millis(0), Millis(1_000), Millis(0));
        assert_eq!(c.to_service(Millis(10)), Some(Millis(0)));
    }

    #[test]
    fn answer_before_question_is_ignored() {
        let mut c = ClockSync::default();
        c.observe(Millis(10), Millis(5), Millis(9));
        assert!(!c.ready());
    }
}
