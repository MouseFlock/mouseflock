//! Which copy is newest, when to fetch it, and what to serve
//! (add-text-clipboard design D1, add-clipboard-images D3, D4). Pure: the host
//! feeds it events and carries out what it returns. Copies follow the cursor:
//! a copy is offered without its contents, and fetched by a machine when the
//! cursor arrives there. Text and images are the same to it: the kind only
//! decides how the contents travel, which is the host's and the node's
//! business.

use std::time::{Duration, Instant};

use engine::FocusView;
use model::PeerId;
use transport::Contents;

/// How long a requested copy may go without any of its bytes arriving. A
/// later arrival may ask again once it has passed.
pub const FETCH_WAIT: Duration = Duration::from_secs(10);

/// Who made a copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Local,
    Member(PeerId),
}

/// One copy, by its owner and the ID its owner gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clip {
    pub owner: Owner,
    pub id: u64,
}

#[derive(Debug, Default)]
pub struct Tracker {
    /// The copy learned of most recently. `None` when this machine's own
    /// newest copy is not shareable.
    newest: Option<Clip>,
    /// This machine's own shareable copy, served on request.
    held: Option<(u64, Contents)>,
    /// The copy on this machine's clipboard.
    holding: Option<Clip>,
    /// A request in flight, and when it gives up unless more bytes arrive.
    fetch: Option<(Clip, Instant)>,
}

impl Tracker {
    /// The agent reported a shareable copy, text or image, given `id`. The
    /// caller offers `id` to every up member.
    pub fn local_copy(&mut self, id: u64, contents: Contents) {
        let clip = Clip {
            owner: Owner::Local,
            id,
        };
        self.newest = Some(clip);
        self.holding = Some(clip);
        self.held = Some((id, contents));
        self.fetch = None;
    }

    /// The agent reported a copy that is not shared. Nothing earlier from
    /// this machine is served any more, and nothing is fetched for it.
    pub fn local_not_shared(&mut self) {
        self.newest = None;
        self.holding = None;
        self.held = None;
        self.fetch = None;
    }

    /// `peer` has a new shareable copy.
    pub fn offer(&mut self, peer: PeerId, id: u64) {
        self.newest = Some(Clip {
            owner: Owner::Member(peer),
            id,
        });
    }

    /// The cursor arrived here. Returns the member and ID to request, if
    /// the newest copy is an up member's that this machine does not hold
    /// and is not already fetching.
    pub fn arrived(&mut self, up: impl Fn(PeerId) -> bool, now: Instant) -> Option<(PeerId, u64)> {
        if self.fetch.is_some_and(|(_, until)| now >= until) {
            self.fetch = None;
        }
        let clip = self.newest?;
        let Owner::Member(peer) = clip.owner else {
            return None;
        };
        if self.holding == Some(clip) || self.fetch.is_some_and(|(c, _)| c == clip) || !up(peer) {
            return None;
        }
        self.fetch = Some((clip, now + FETCH_WAIT));
        Some((peer, clip.id))
    }

    /// `peer` asked for `id`: the contents, if that is the copy held here.
    /// Cheap: the contents are shared, not copied.
    pub fn request(&self, id: u64) -> Option<Contents> {
        self.held
            .as_ref()
            .filter(|(held, _)| *held == id)
            .map(|(_, contents)| contents.clone())
    }

    /// Bytes of `peer`'s copy `id` are still arriving: if that is the copy
    /// being fetched, it gets another [`FETCH_WAIT`] from `now`.
    pub fn progress(&mut self, peer: PeerId, id: u64, now: Instant) {
        let clip = Clip {
            owner: Owner::Member(peer),
            id,
        };
        if let Some((fetching, until)) = &mut self.fetch
            && *fetching == clip
            && now < *until
        {
            *until = now + FETCH_WAIT;
        }
    }

    /// A copy arrived from `peer` at `now`. Returns it for the agent to set
    /// if it is the copy being fetched, the fetch has not given up, and it is
    /// still the newest; otherwise it is dropped.
    pub fn received(
        &mut self,
        peer: PeerId,
        id: u64,
        contents: Contents,
        now: Instant,
    ) -> Option<Contents> {
        let clip = Clip {
            owner: Owner::Member(peer),
            id,
        };
        let (fetching, until) = self.fetch?;
        if fetching != clip || now >= until {
            if now >= until {
                self.fetch = None;
            }
            return None;
        }
        if self.newest != Some(clip) {
            return None;
        }
        self.fetch = None;
        self.holding = Some(clip);
        Some(contents)
    }

    /// `peer` went down: a fetch from it is abandoned.
    pub fn peer_down(&mut self, peer: PeerId) {
        if self
            .fetch
            .is_some_and(|(c, _)| c.owner == Owner::Member(peer))
        {
            self.fetch = None;
        }
    }
}

/// Whether a focus change means the cursor arrived at this machine: it
/// became controlled by a member, or came back from forwarding.
pub fn arrived(previous: FocusView, next: FocusView) -> bool {
    match (previous, next) {
        (FocusView::Controlled(a), FocusView::Controlled(b)) => a != b,
        (_, FocusView::Controlled(_)) => true,
        (FocusView::Forwarding(_), FocusView::Local) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: PeerId = PeerId([1; 32]);
    const B: PeerId = PeerId([2; 32]);

    fn all(_: PeerId) -> bool {
        true
    }

    fn text(s: &str) -> Contents {
        Contents::Text(std::sync::Arc::from(s))
    }

    fn image(bytes: &[u8]) -> Contents {
        Contents::Image(std::sync::Arc::from(bytes))
    }

    #[test]
    fn copy_then_arrival_fetches_once() {
        let mut t = Tracker::default();
        let now = Instant::now();
        t.offer(A, 7);
        assert_eq!(t.arrived(all, now), Some((A, 7)));
        // Already fetching: a second arrival asks nothing.
        assert_eq!(t.arrived(all, now), None);
        assert_eq!(t.received(A, 7, text("hello"), now), Some(text("hello")));
        // Holding it now: nothing to fetch.
        assert_eq!(t.arrived(all, now), None);
    }

    #[test]
    fn copies_on_two_machines() {
        let mut t = Tracker::default();
        t.offer(A, 1);
        t.offer(B, 2);
        assert_eq!(t.arrived(all, Instant::now()), Some((B, 2)));
    }

    #[test]
    fn own_copy_after_an_offer() {
        let mut t = Tracker::default();
        t.offer(A, 1);
        t.local_copy(9, text("mine"));
        assert_eq!(t.arrived(all, Instant::now()), None);
        assert_eq!(t.request(9), Some(text("mine")));
    }

    #[test]
    fn nothing_newer() {
        let mut t = Tracker::default();
        assert_eq!(t.arrived(all, Instant::now()), None);
        t.local_copy(1, text("x"));
        assert_eq!(t.arrived(all, Instant::now()), None);
    }

    #[test]
    fn owner_down() {
        let mut t = Tracker::default();
        t.offer(A, 1);
        assert_eq!(t.arrived(|p| p != A, Instant::now()), None);
        assert_eq!(t.arrived(all, Instant::now()), Some((A, 1)));
    }

    #[test]
    fn stale_request_gets_nothing() {
        let mut t = Tracker::default();
        t.local_copy(1, text("old"));
        t.local_copy(2, text("new"));
        assert_eq!(t.request(1), None);
        assert_eq!(t.request(2), Some(text("new")));
    }

    #[test]
    fn private_after_shareable() {
        let mut t = Tracker::default();
        t.local_copy(1, text("hello"));
        t.local_not_shared();
        assert_eq!(t.request(1), None);
        assert_eq!(t.arrived(all, Instant::now()), None);
    }

    #[test]
    fn no_echo() {
        // B fetched A's copy; nothing is offered back, and B holds it.
        let mut t = Tracker::default();
        t.offer(A, 5);
        t.arrived(all, Instant::now());
        t.received(A, 5, text("hi"), Instant::now());
        assert_eq!(t.arrived(all, Instant::now()), None);
        assert_eq!(t.request(5), None, "only own copies are served");
    }

    #[test]
    fn newer_copy_during_a_transfer() {
        let mut t = Tracker::default();
        t.offer(A, 1);
        t.arrived(all, Instant::now());
        t.local_copy(2, text("mine"));
        assert_eq!(t.received(A, 1, text("theirs"), Instant::now()), None);

        let mut u = Tracker::default();
        u.offer(A, 1);
        u.arrived(all, Instant::now());
        u.offer(B, 2);
        assert_eq!(u.received(A, 1, text("older"), Instant::now()), None);
        assert_eq!(u.arrived(all, Instant::now()), Some((B, 2)));
    }

    #[test]
    fn unrequested_text_is_dropped() {
        let mut t = Tracker::default();
        t.offer(A, 1);
        let now = Instant::now();
        assert_eq!(t.received(A, 1, text("x"), now), None);
        t.arrived(all, now);
        assert_eq!(t.received(B, 1, text("x"), now), None);
        assert_eq!(t.received(A, 2, text("x"), now), None);
    }

    #[test]
    fn fetch_timeout_then_retry() {
        let mut t = Tracker::default();
        let now = Instant::now();
        t.offer(A, 1);
        assert_eq!(t.arrived(all, now), Some((A, 1)));
        assert_eq!(
            t.arrived(all, now + FETCH_WAIT - Duration::from_millis(1)),
            None
        );
        assert_eq!(t.arrived(all, now + FETCH_WAIT), Some((A, 1)));
    }

    #[test]
    fn steady_progress_keeps_a_fetch_alive() {
        // A large image that takes 25 s, with bytes every few seconds.
        let mut t = Tracker::default();
        let start = Instant::now();
        t.offer(A, 3);
        assert_eq!(t.arrived(all, start), Some((A, 3)));
        for s in (3..=24).step_by(3) {
            t.progress(A, 3, start + Duration::from_secs(s));
        }
        let done = start + Duration::from_secs(25);
        // Still fetching: a new arrival does not ask again.
        assert_eq!(t.arrived(all, done), None);
        assert_eq!(t.received(A, 3, image(b"png"), done), Some(image(b"png")));
    }

    #[test]
    fn ten_seconds_of_silence_gives_up() {
        let mut t = Tracker::default();
        let start = Instant::now();
        t.offer(A, 3);
        t.arrived(all, start);
        t.progress(A, 3, start + Duration::from_secs(5));
        let silent = start + Duration::from_secs(15);
        // Too late: the copy is dropped, and the next arrival asks again.
        assert_eq!(t.received(A, 3, image(b"png"), silent), None);
        assert_eq!(t.arrived(all, silent), Some((A, 3)));
        // Progress after giving up revives nothing.
        let mut u = Tracker::default();
        u.offer(A, 4);
        u.arrived(all, start);
        u.progress(A, 4, start + FETCH_WAIT);
        assert_eq!(u.received(A, 4, image(b"png"), start + FETCH_WAIT), None);
    }

    #[test]
    fn progress_for_another_copy_changes_nothing() {
        let mut t = Tracker::default();
        let start = Instant::now();
        t.offer(A, 3);
        t.arrived(all, start);
        t.progress(A, 9, start + Duration::from_secs(9));
        t.progress(B, 3, start + Duration::from_secs(9));
        assert_eq!(t.arrived(all, start + FETCH_WAIT), Some((A, 3)));
    }

    #[test]
    fn newest_copy_wins_across_kinds() {
        let mut t = Tracker::default();
        let now = Instant::now();
        t.local_copy(1, image(b"png"));
        t.offer(B, 2);
        assert_eq!(t.arrived(all, now), Some((B, 2)));
        assert_eq!(t.received(B, 2, text("words"), now), Some(text("words")));
        // An image offered after text is the newest in turn.
        t.offer(A, 3);
        assert_eq!(t.arrived(all, now), Some((A, 3)));
        // And this machine's own image is served by ID, like text.
        t.local_copy(4, image(b"mine"));
        assert_eq!(t.request(4), Some(image(b"mine")));
        assert_eq!(t.request(1), None);
    }

    #[test]
    fn peer_down_abandons_its_fetch() {
        let mut t = Tracker::default();
        t.offer(A, 1);
        t.arrived(all, Instant::now());
        t.peer_down(B);
        assert_eq!(
            t.arrived(all, Instant::now()),
            None,
            "still fetching from A"
        );
        t.peer_down(A);
        assert_eq!(t.arrived(all, Instant::now()), Some((A, 1)));
    }

    #[test]
    fn arrivals() {
        use FocusView::{Controlled, Forwarding, Local};
        assert!(arrived(Local, Controlled(A)));
        assert!(arrived(Forwarding(A), Controlled(B)));
        assert!(arrived(Controlled(A), Controlled(B)));
        assert!(arrived(Forwarding(A), Local));
        assert!(!arrived(Local, Local));
        assert!(!arrived(Local, Forwarding(A)));
        assert!(!arrived(Controlled(A), Local));
        assert!(!arrived(Controlled(A), Controlled(A)));
        assert!(!arrived(Controlled(A), Forwarding(B)));
        assert!(!arrived(Forwarding(A), Forwarding(B)));
    }
}
