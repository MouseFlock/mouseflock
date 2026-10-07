//! Which agent carries this machine's input (add-winlogon-agent design D8,
//! spec secure-desktop). Pure: the host tells it which links exist and what
//! the agents report, and carries out what it returns.
//!
//! The winlogon agent is active while its latest report says its desktop
//! is the input desktop; otherwise the session agent is. Injection and
//! warps go to the active agent, and the engine takes input and the screen
//! only from it. Capture modes go to every agent, and clipboard text only
//! to the session agent; the host does both without asking.

use ipc::link::AgentRole;
use model::Screen;

/// What the host must do after a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Give the engine this screen.
    Screen(Screen),
    /// Tell the engine local input is lost.
    LocalInputLost,
    /// The active agent changed, for the log. `secure_inputs` is how many
    /// input events the winlogon agent reported while it was active, when
    /// it stops being active.
    Active {
        now: AgentRole,
        secure_inputs: Option<u64>,
    },
}

#[derive(Debug, Default)]
pub struct Routing {
    session: Slot,
    winlogon: Slot,
    /// The winlogon agent's latest report.
    secure: bool,
    /// The screen the engine has.
    given: Option<Screen>,
    /// Input events taken from the winlogon agent while active.
    secure_inputs: u64,
}

#[derive(Debug, Default)]
struct Slot {
    linked: bool,
    screen: Option<Screen>,
}

impl Routing {
    /// The agent on the input desktop.
    pub fn active(&self) -> AgentRole {
        if self.secure && self.winlogon.linked {
            AgentRole::Winlogon
        } else {
            AgentRole::Session
        }
    }

    fn slot(&mut self, role: AgentRole) -> &mut Slot {
        match role {
            AgentRole::Session => &mut self.session,
            AgentRole::Winlogon => &mut self.winlogon,
        }
    }

    /// An agent of `role` linked.
    pub fn linked(&mut self, role: AgentRole) {
        *self.slot(role) = Slot {
            linked: true,
            screen: None,
        };
    }

    /// The link of `role` ended.
    pub fn ended(&mut self, role: AgentRole) -> Vec<Effect> {
        if !self.slot(role).linked {
            return Vec::new();
        }
        let before = self.active();
        *self.slot(role) = Slot::default();
        if role == AgentRole::Winlogon {
            self.secure = false;
        }
        let mut out = Vec::new();
        if before == role {
            out.push(Effect::LocalInputLost);
        }
        out.extend(self.switched(before));
        out
    }

    /// Whether input from `role` goes to the engine: only from the active
    /// agent.
    pub fn takes_input(&mut self, role: AgentRole) -> bool {
        let takes = role == self.active();
        if takes && role == AgentRole::Winlogon {
            self.secure_inputs += 1;
        }
        takes
    }

    /// `role` reported its screen.
    pub fn screen(&mut self, role: AgentRole, screen: Screen) -> Vec<Effect> {
        self.slot(role).screen = Some(screen);
        if role == self.active() {
            self.give(role).into_iter().collect()
        } else {
            Vec::new()
        }
    }

    /// The winlogon agent reported whether its desktop is the input
    /// desktop.
    pub fn input_desktop(&mut self, active: bool) -> Vec<Effect> {
        let before = self.active();
        self.secure = active;
        self.switched(before)
    }

    /// The effects of the active agent having been `before`.
    fn switched(&mut self, before: AgentRole) -> Vec<Effect> {
        let now = self.active();
        if now == before {
            return Vec::new();
        }
        let secure_inputs =
            (before == AgentRole::Winlogon).then(|| std::mem::take(&mut self.secure_inputs));
        if now == AgentRole::Winlogon {
            self.secure_inputs = 0;
        }
        let mut out = vec![Effect::Active { now, secure_inputs }];
        out.extend(self.give(now));
        out
    }

    /// `role`'s screen, if it has one the engine does not.
    fn give(&mut self, role: AgentRole) -> Option<Effect> {
        let screen = self.slot(role).screen.clone()?;
        if self.given.as_ref() == Some(&screen) {
            return None;
        }
        self.given = Some(screen.clone());
        Some(Effect::Screen(screen))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AgentRole::{Session, Winlogon};
    use model::{Monitor, MonitorId, Rect};

    fn screen(width: i32) -> Screen {
        Screen::new(vec![Monitor {
            id: MonitorId("M".into()),
            rect: Rect::new(0, 0, width, 1080),
            dpi: 96,
        }])
    }

    fn active(now: AgentRole, secure_inputs: Option<u64>) -> Effect {
        Effect::Active { now, secure_inputs }
    }

    /// Both agents linked with the same screen, the session agent active.
    fn both() -> Routing {
        let mut r = Routing::default();
        r.linked(Session);
        r.linked(Winlogon);
        assert_eq!(
            r.screen(Session, screen(1920)),
            [Effect::Screen(screen(1920))]
        );
        assert_eq!(r.screen(Winlogon, screen(1920)), []);
        assert_eq!(r.input_desktop(false), []);
        r
    }

    #[test]
    fn session_agent_is_active_by_default() {
        let mut r = Routing::default();
        assert_eq!(r.active(), Session);
        r.linked(Winlogon);
        assert_eq!(r.active(), Session, "linked but not yet reporting");
    }

    #[test]
    fn uac_prompt() {
        let mut r = both();
        assert_eq!(r.input_desktop(true), [active(Winlogon, None)]);
        assert_eq!(r.active(), Winlogon);
        // The same screen is not given again.
    }

    #[test]
    fn prompt_answered() {
        let mut r = both();
        r.input_desktop(true);
        assert!(r.takes_input(Winlogon));
        assert!(r.takes_input(Winlogon));
        assert_eq!(r.input_desktop(false), [active(Session, Some(2))]);
        assert_eq!(r.active(), Session);
    }

    #[test]
    fn repeated_reports_change_nothing() {
        let mut r = both();
        assert_eq!(r.input_desktop(true), [active(Winlogon, None)]);
        assert_eq!(r.input_desktop(true), []);
    }

    #[test]
    fn winlogon_agent_lost() {
        let mut r = both();
        r.input_desktop(true);
        assert_eq!(
            r.ended(Winlogon),
            [Effect::LocalInputLost, active(Session, Some(0))]
        );
        assert_eq!(r.active(), Session);
        // A new winlogon agent starts inactive until it reports.
        r.linked(Winlogon);
        assert_eq!(r.active(), Session);
    }

    #[test]
    fn input_from_the_inactive_agent() {
        let mut r = both();
        assert!(r.takes_input(Session));
        assert!(!r.takes_input(Winlogon));
        r.input_desktop(true);
        assert!(!r.takes_input(Session));
        assert!(r.takes_input(Winlogon));
    }

    #[test]
    fn only_the_winlogon_agent_at_boot() {
        let mut r = Routing::default();
        r.linked(Winlogon);
        // Its first reports: the screen, then that its desktop has the
        // input.
        assert_eq!(r.screen(Winlogon, screen(1920)), []);
        assert_eq!(
            r.input_desktop(true),
            [active(Winlogon, None), Effect::Screen(screen(1920))]
        );
        assert!(r.takes_input(Winlogon));
        // Logon: the session agent links and reports; the secure desktop
        // still has the input until the winlogon agent says otherwise.
        r.linked(Session);
        assert_eq!(r.screen(Session, screen(1920)), []);
        assert_eq!(r.input_desktop(false), [active(Session, Some(1))]);
    }

    #[test]
    fn session_agent_restarts_during_a_uac_prompt() {
        let mut r = both();
        r.input_desktop(true);
        assert_eq!(r.ended(Session), []);
        r.linked(Session);
        assert_eq!(r.screen(Session, screen(1920)), []);
        assert_eq!(r.active(), Winlogon);
    }

    #[test]
    fn session_agent_lost_while_active() {
        let mut r = both();
        assert_eq!(r.ended(Session), [Effect::LocalInputLost]);
        assert_eq!(r.ended(Session), [], "already gone");
    }

    #[test]
    fn active_agent_screen_changes_reach_the_engine() {
        let mut r = both();
        assert_eq!(
            r.screen(Session, screen(2560)),
            [Effect::Screen(screen(2560))]
        );
        // The inactive agent's screen waits until it is active.
        assert_eq!(r.screen(Winlogon, screen(1280)), []);
        assert_eq!(
            r.input_desktop(true),
            [active(Winlogon, None), Effect::Screen(screen(1280))]
        );
        assert_eq!(
            r.input_desktop(false),
            [active(Session, Some(0)), Effect::Screen(screen(2560))]
        );
    }

    #[test]
    fn agents_link_at_different_times() {
        // The capture mode goes to every agent; the host sends it on each
        // link. Routing only needs both to be tracked independently.
        let mut r = Routing::default();
        r.linked(Session);
        r.screen(Session, screen(1920));
        r.linked(Winlogon);
        assert_eq!(r.active(), Session);
        assert_eq!(r.ended(Winlogon), []);
        assert_eq!(r.active(), Session);
    }
}
