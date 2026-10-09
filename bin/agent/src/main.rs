//! `deskhop-agent.exe`: input and clipboard on one desktop.
//!
//! `--session --pipe <name>` runs on the user desktop, and
//! `--winlogon --pipe <name>` on the secure desktop for UAC prompts, the
//! lock screen and the logon screen. Each talks to the service over the
//! named pipe it is given (see `link`). Spawned by the service only. Never
//! touches the network.
//!
//! `--trace [--withhold mouse|all <seconds>]` is a developer diagnostic: it
//! prints every captured event, screen report and input desktop change to
//! the console. With `--withhold` it withholds physical input for 1 to 10
//! seconds and then passes everything again.

#![forbid(unsafe_code)]

mod files;
mod link;

use std::process::ExitCode;
use std::time::{Duration, Instant};

use ipc::link::AgentRole;
use model::{CaptureMode, InputEvent, Origin, Screen};
use win32_input::{Captured, MonitorDetail};

/// The longest `--withhold` accepts, so a trace can never keep the user's
/// input for long.
const MAX_WITHHOLD_SECS: u64 = 10;

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Link {
        role: AgentRole,
        pipe: String,
    },
    Trace {
        withhold: Option<(CaptureMode, u64)>,
    },
}

fn parse_mode(args: impl Iterator<Item = String>) -> Option<Mode> {
    let args: Vec<String> = args.collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [kind, "--pipe", pipe] if !pipe.is_empty() => {
            let role = match *kind {
                "--session" => AgentRole::Session,
                "--winlogon" => AgentRole::Winlogon,
                _ => return None,
            };
            Some(Mode::Link {
                role,
                pipe: (*pipe).to_owned(),
            })
        }
        ["--trace"] => Some(Mode::Trace { withhold: None }),
        ["--trace", "--withhold", what, secs] => {
            let mode = match *what {
                "mouse" => CaptureMode::WithholdMouse,
                "all" => CaptureMode::WithholdAll,
                _ => return None,
            };
            let secs: u64 = secs.parse().ok()?;
            (1..=MAX_WITHHOLD_SECS)
                .contains(&secs)
                .then_some(Mode::Trace {
                    withhold: Some((mode, secs)),
                })
        }
        _ => None,
    }
}

fn main() -> ExitCode {
    match parse_mode(std::env::args().skip(1)) {
        Some(Mode::Link { role, pipe }) => link::run(&pipe, role),
        Some(Mode::Trace { withhold }) => trace(withhold),
        None => {
            eprintln!(
                "usage: deskhop-agent.exe --session|--winlogon --pipe <name>\n       \
                 deskhop-agent.exe --trace [--withhold mouse|all <1..{MAX_WITHHOLD_SECS}>]"
            );
            ExitCode::from(2)
        }
    }
}

/// Prints captured events until the process is ended (Ctrl+C).
fn trace(withhold: Option<(CaptureMode, u64)>) -> ExitCode {
    let options = win32_input::Options {
        report_input_desktop: true,
    };
    let (capture, rx) = match win32_input::start(options) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("deskhop-agent: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut until = None;
    if let Some((mode, secs)) = withhold {
        capture.set_mode(mode);
        until = Some(Instant::now() + Duration::from_secs(secs));
        println!("withholding ({mode:?}) for {secs} s");
    }
    loop {
        let wait = until.map_or(Duration::from_secs(3600), |u: Instant| {
            u.saturating_duration_since(Instant::now())
        });
        match rx.recv_timeout(wait) {
            Ok(c) => println!("{}", describe(&c)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return ExitCode::FAILURE,
        }
        if until.is_some_and(|u| Instant::now() >= u) {
            capture.set_mode(CaptureMode::PassAll);
            until = None;
            println!("passing all input again");
        }
    }
}

fn describe(c: &Captured) -> String {
    match c {
        Captured::Input { at, input, origin } => {
            let origin = match origin {
                Origin::Physical => "physical",
                Origin::Injected => "injected",
            };
            let what = match input {
                InputEvent::Key { key, down } => {
                    format!("key {:#04x} {}", key.0, if *down { "down" } else { "up" })
                }
                InputEvent::Button { button, down } => {
                    format!("button {button:?} {}", if *down { "down" } else { "up" })
                }
                InputEvent::Wheel { dx, dy } => format!("wheel {dx} {dy}"),
                InputEvent::Motion { dx, dy, cursor } => {
                    format!("motion {dx} {dy} at ({}, {})", cursor.x, cursor.y)
                }
            };
            format!("{:>10} {what} {origin}", at.0)
        }
        Captured::Screen(screen, details) => describe_screen(screen, details),
        Captured::InputDesktop(true) => "input desktop: this one".to_owned(),
        Captured::InputDesktop(false) => {
            "input desktop: another one, or one this process may not open".to_owned()
        }
    }
}

fn describe_screen(screen: &Screen, details: &[MonitorDetail]) -> String {
    let mut out = String::from("screen:");
    for (m, d) in screen.monitors.iter().zip(details) {
        let r = m.rect;
        out += &format!(
            "\n  {} at ({}, {}) {}x{} dpi {} [{}]",
            m.id.0, r.x, r.y, r.w, r.h, m.dpi, d.gdi_name
        );
        for t in &d.targets {
            let edid = t.edid.as_ref().map_or("no EDID".into(), |e| {
                format!(
                    "EDID {}{:04X} serial {}",
                    e.manufacturer,
                    e.product,
                    e.serial.as_deref().unwrap_or("none")
                )
            });
            out += &format!("\n    target {} ({edid}) -> {}", t.path, t.id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Option<Mode> {
        parse_mode(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn valid_forms() {
        assert_eq!(
            parse(&["--session", "--pipe", r"\\.\pipe\x"]),
            Some(Mode::Link {
                role: AgentRole::Session,
                pipe: r"\\.\pipe\x".into()
            })
        );
        assert_eq!(
            parse(&["--winlogon", "--pipe", r"\\.\pipe\y"]),
            Some(Mode::Link {
                role: AgentRole::Winlogon,
                pipe: r"\\.\pipe\y".into()
            })
        );
        assert_eq!(parse(&["--trace"]), Some(Mode::Trace { withhold: None }));
        assert_eq!(
            parse(&["--trace", "--withhold", "mouse", "5"]),
            Some(Mode::Trace {
                withhold: Some((CaptureMode::WithholdMouse, 5))
            })
        );
        assert_eq!(
            parse(&["--trace", "--withhold", "all", "10"]),
            Some(Mode::Trace {
                withhold: Some((CaptureMode::WithholdAll, 10))
            })
        );
    }

    #[test]
    fn withhold_is_bounded() {
        assert_eq!(parse(&["--trace", "--withhold", "all", "0"]), None);
        assert_eq!(parse(&["--trace", "--withhold", "all", "11"]), None);
        assert_eq!(parse(&["--trace", "--withhold", "all", "-1"]), None);
        assert_eq!(parse(&["--trace", "--withhold", "all"]), None);
    }

    #[test]
    fn unknown_arguments() {
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&["--trace", "--withhold", "keys", "5"]), None);
        assert_eq!(parse(&["--trace", "--verbose"]), None);
        assert_eq!(parse(&["--session", "--trace"]), None);
        assert_eq!(parse(&["--session"]), None);
        assert_eq!(parse(&["--session", "--pipe"]), None);
        assert_eq!(parse(&["--session", "--pipe", ""]), None);
        assert_eq!(parse(&["--winlogon"]), None);
        assert_eq!(parse(&["--winlogon", "--pipe"]), None);
        assert_eq!(parse(&["--winlogon", "--pipe", ""]), None);
        assert_eq!(parse(&["--trace", "--pipe", r"\\.\pipe\x"]), None);
    }
}
