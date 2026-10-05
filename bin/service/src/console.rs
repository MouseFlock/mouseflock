//! Console mode's commands and output (design D11). Pairing happens here
//! until the UI exists. Codes and reports go to stdout; logs go to stderr.

use engine::FocusView;
use model::PeerId;
use pairing::{Code, JoinFailure, ModeClosed};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use transport::Down;

use crate::host::{HostCommand, PeerRow, Report};
use crate::store::{hex, side_name};

pub const HELP: &str = "\
commands:
  pair             open pairing mode and show a code
  close            close pairing mode
  join <code>      pair with the machine showing <code>
  peers            list members and whether they are up
  remove <peer>    remove a member, by name or key prefix
  undo             take back the side just learned
  forget-layout    forget every learned side
  quit             stop";

#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    Pair,
    Close,
    Join(Code),
    Peers,
    Remove(String),
    Undo,
    ForgetLayout,
    Quit,
    Help,
    Empty,
}

pub fn parse(line: &str) -> Result<Line, String> {
    let mut words = line.split_whitespace();
    let Some(command) = words.next() else {
        return Ok(Line::Empty);
    };
    let rest: Vec<&str> = words.collect();
    let none = |l: Line| {
        if rest.is_empty() {
            Ok(l)
        } else {
            Err(format!("`{command}` takes no arguments"))
        }
    };
    match command.to_ascii_lowercase().as_str() {
        "pair" => none(Line::Pair),
        "close" => none(Line::Close),
        "peers" => none(Line::Peers),
        "undo" => none(Line::Undo),
        "forget-layout" => none(Line::ForgetLayout),
        "quit" | "exit" => none(Line::Quit),
        "help" | "?" => none(Line::Help),
        "join" => match rest.as_slice() {
            [code] => Code::parse(code).map(Line::Join).map_err(|e| e.to_string()),
            _ => Err("usage: join <code>".into()),
        },
        "remove" => match rest.as_slice() {
            [peer] => Ok(Line::Remove((*peer).to_owned())),
            _ => Err("usage: remove <peer>".into()),
        },
        other => Err(format!("unknown command `{other}`; type `help`")),
    }
}

/// The member `query` names: its full machine name (any case), or else a
/// prefix of its key in hex, at least 2 characters.
pub fn resolve(rows: &[PeerRow], query: &str) -> Result<PeerId, String> {
    let by_name: Vec<&PeerRow> = rows
        .iter()
        .filter(|r| r.name.eq_ignore_ascii_case(query))
        .collect();
    let matches = if by_name.is_empty() {
        let q = query.to_ascii_lowercase();
        if q.len() < 2 {
            return Err(format!(
                "`{query}` is too short; give a name or at least 2 hex digits"
            ));
        }
        rows.iter()
            .filter(|r| hex(&r.peer.0).starts_with(&q))
            .collect()
    } else {
        by_name
    };
    match matches.as_slice() {
        [one] => Ok(one.peer),
        [] => Err(format!("no member is called `{query}`")),
        _ => Err(format!(
            "`{query}` matches more than one member; give more of the key"
        )),
    }
}

fn short(peer: PeerId) -> String {
    hex(&peer.0[..4])
}

fn join_failure(f: JoinFailure) -> &'static str {
    match f {
        JoinFailure::WrongCode => "the code was wrong",
        JoinFailure::NotReady => "that machine is not in pairing mode, or is busy",
        JoinFailure::ProtocolError => "the other machine broke the protocol",
        JoinFailure::DeskFull => "this desk is full",
        JoinFailure::ConnectionLost => "the connection was lost",
        JoinFailure::Unreachable => "no machine answered at the code's address",
    }
}

/// One line for a report.
pub fn describe(report: &Report) -> String {
    match report {
        Report::PairingClosed(why) => match why {
            ModeClosed::Paired => "pairing closed: a machine joined".into(),
            ModeClosed::Expired => "pairing closed: 10 minutes passed".into(),
            ModeClosed::ByUser => "pairing closed".into(),
            ModeClosed::TooManyFailures => {
                "pairing closed: too many wrong codes; type `pair` for a new code".into()
            }
        },
        Report::Paired { peer, name } => format!("paired with {name} ({})", short(*peer)),
        Report::JoinFailed(f) => format!("join failed: {}", join_failure(*f)),
        Report::PeerUp { name, .. } => format!("{name} is up"),
        Report::PeerDown { name, why, .. } => format!(
            "{name} is down ({})",
            match why {
                Down::Lost => "lost",
                Down::Goodbye => "said goodbye",
                Down::Removed => "removed",
            }
        ),
        Report::Focus { focus, name } => {
            let name = name.as_deref().unwrap_or("?");
            match focus {
                FocusView::Local => "focus: here".into(),
                FocusView::Forwarding(_) => format!("focus: on {name}"),
                FocusView::Controlled(_) => format!("focus: {name} controls this machine"),
            }
        }
        Report::Learned {
            name,
            side,
            undo_secs,
            ..
        } => format!(
            "learned: {name} is to the {}; type `undo` within {undo_secs} s to take it back",
            side_name(*side)
        ),
        Report::AgentLinked => "agent linked".into(),
        Report::AgentGone => "agent gone".into(),
    }
}

async fn peers(host: &mpsc::UnboundedSender<HostCommand>) -> Vec<PeerRow> {
    let (reply, rows) = oneshot::channel();
    let _ = host.send(HostCommand::Peers(reply));
    rows.await.unwrap_or_default()
}

/// Reads commands until `quit` or end of input, printing reports as they
/// come.
pub async fn run(
    host: mpsc::UnboundedSender<HostCommand>,
    mut reports: mpsc::UnboundedReceiver<Report>,
) {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            Some(report) = reports.recv() => println!("{}", describe(&report)),
            line = lines.next_line() => {
                let Ok(Some(line)) = line else { return };
                match parse(&line) {
                    Err(e) => println!("{e}"),
                    Ok(Line::Empty) => {}
                    Ok(Line::Help) => println!("{HELP}"),
                    Ok(Line::Quit) => return,
                    Ok(Line::Pair) => {
                        let (reply, code) = oneshot::channel();
                        let _ = host.send(HostCommand::OpenPairing(reply));
                        match code.await {
                            Ok(Ok(code)) => println!("pairing code: {code}  (valid for 10 minutes)"),
                            Ok(Err(e)) => println!("cannot open pairing: {e}"),
                            Err(_) => return,
                        }
                    }
                    Ok(Line::Close) => {
                        let _ = host.send(HostCommand::ClosePairing);
                    }
                    Ok(Line::Join(code)) => {
                        println!("joining…");
                        let _ = host.send(HostCommand::Join(code));
                    }
                    Ok(Line::Peers) => {
                        let rows = peers(&host).await;
                        if rows.is_empty() {
                            println!("no members yet; use `pair` here and `join` on the other machine");
                        }
                        for r in rows {
                            println!(
                                "  {:<20} {}  {}",
                                r.name,
                                short(r.peer),
                                if r.up { "up" } else { "down" }
                            );
                        }
                    }
                    Ok(Line::Remove(query)) => match resolve(&peers(&host).await, &query) {
                        Ok(peer) => {
                            let _ = host.send(HostCommand::Remove(peer));
                        }
                        Err(e) => println!("{e}"),
                    },
                    Ok(Line::Undo) => {
                        let _ = host.send(HostCommand::Undo);
                    }
                    Ok(Line::ForgetLayout) => {
                        let _ = host.send(HostCommand::ForgetLayout);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, first: u8) -> PeerRow {
        let mut key = [0x11; 32];
        key[0] = first;
        PeerRow {
            peer: PeerId(key),
            name: name.into(),
            up: true,
        }
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse("  pair "), Ok(Line::Pair));
        assert_eq!(parse("PEERS"), Ok(Line::Peers));
        assert_eq!(parse("forget-layout"), Ok(Line::ForgetLayout));
        assert_eq!(parse(""), Ok(Line::Empty));
        assert_eq!(
            parse("remove LAPTOP-7"),
            Ok(Line::Remove("LAPTOP-7".into()))
        );
        assert!(parse("pair now").is_err());
        assert!(parse("join").is_err());
        assert!(parse("remove").is_err());
        assert!(parse("dance").unwrap_err().contains("unknown"));
        // A code with a typo is caught by its check digit.
        assert!(parse("join 1-0000000").is_err());
    }

    #[test]
    fn resolves_peers() {
        let rows = [row("LAPTOP-7", 0xab), row("Desk", 0xac), row("Other", 0x12)];
        assert_eq!(resolve(&rows, "laptop-7"), Ok(rows[0].peer));
        assert_eq!(resolve(&rows, "AB"), Ok(rows[0].peer));
        assert_eq!(resolve(&rows, "12"), Ok(rows[2].peer));
        assert!(resolve(&rows, "a").unwrap_err().contains("too short"));
        assert!(resolve(&rows, "nobody").unwrap_err().contains("no member"));
    }

    #[test]
    fn ambiguous_prefix_changes_nothing() {
        let rows = [row("One", 0xab), row("Two", 0xab)];
        assert!(resolve(&rows, "ab").unwrap_err().contains("more than one"));
    }
}
