//! mDNS: advertising this machine and turning other machines' advertisements
//! into sightings (spec: discovery; design D10).
//!
//! A sighting is a hint, never trust: dialing pins the identity it names.

use std::collections::HashMap;
use std::net::Ipv4Addr;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use model::PeerId;

/// The DNS-SD service type.
pub const SERVICE_TYPE: &str = "_mouseflock._udp.local.";

/// One machine seen on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    pub id: PeerId,
    pub addrs: Vec<Ipv4Addr>,
    pub port: u16,
    /// Its pairing mode is open.
    pub pairing: bool,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The instance name: the first 16 bytes of the identity in lowercase hex.
pub fn instance_name(id: &PeerId) -> String {
    hex(&id.0[..16])
}

/// The TXT entries: `id`, and `pair=1` while pairing mode is open.
pub fn txt(id: &PeerId, pairing: bool) -> Vec<(String, String)> {
    let mut entries = vec![("id".to_owned(), hex(&id.0))];
    if pairing {
        entries.push(("pair".to_owned(), "1".to_owned()));
    }
    entries
}

/// The identity in a TXT `id` value: exactly 64 lowercase hex digits.
pub fn parse_id(value: &str) -> Option<PeerId> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    let mut id = [0; 32];
    for (i, byte) in id.iter_mut().enumerate() {
        *byte = u8::from_str_radix(value.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(PeerId(id))
}

/// A sighting from a resolved advertisement's fields, or `None` if its `id`
/// is missing or malformed.
pub fn sighting(
    id: Option<&str>,
    pair: Option<&str>,
    addrs: impl IntoIterator<Item = Ipv4Addr>,
    port: u16,
) -> Option<Sighting> {
    Some(Sighting {
        id: parse_id(id?)?,
        addrs: addrs.into_iter().collect(),
        port,
        pairing: pair == Some("1"),
    })
}

/// The running mDNS responder and browser.
pub struct Mdns {
    daemon: ServiceDaemon,
    id: PeerId,
    port: u16,
}

impl Mdns {
    /// Starts advertising `id` on `port` and browsing. Every resolved
    /// advertisement other than this machine's own is passed to `seen`.
    pub fn start(
        id: PeerId,
        port: u16,
        seen: impl Fn(Sighting) + Send + 'static,
    ) -> Result<Self, mdns_sd::Error> {
        let daemon = ServiceDaemon::new()?;
        let mdns = Self { daemon, id, port };
        mdns.advertise(false)?;
        let events = mdns.daemon.browse(SERVICE_TYPE)?;
        std::thread::Builder::new()
            .name("mouseflock-mdns".to_owned())
            .spawn(move || {
                while let Ok(event) = events.recv() {
                    if let ServiceEvent::ServiceResolved(info) = event {
                        let found = sighting(
                            info.get_property_val_str("id"),
                            info.get_property_val_str("pair"),
                            info.get_addresses_v4(),
                            info.get_port(),
                        );
                        if let Some(found) = found.filter(|s| s.id != id) {
                            seen(found);
                        }
                    }
                }
            })
            .map_err(|e| mdns_sd::Error::Msg(e.to_string()))?;
        Ok(mdns)
    }

    /// Advertises again, with `pair=1` while `pairing`.
    pub fn advertise(&self, pairing: bool) -> Result<(), mdns_sd::Error> {
        let name = instance_name(&self.id);
        let host = format!("mouseflock-{name}.local.");
        let properties: HashMap<String, String> = txt(&self.id, pairing).into_iter().collect();
        let info = ServiceInfo::new(SERVICE_TYPE, &name, &host, "", self.port, properties)?
            .enable_addr_auto();
        self.daemon.register(info)
    }
}

impl Drop for Mdns {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> PeerId {
        let mut id = [0; 32];
        for (i, b) in id.iter_mut().enumerate() {
            *b = 0xab_u8.wrapping_add(i as u8);
        }
        PeerId(id)
    }

    #[test]
    fn advertised_identity() {
        let id = id();
        assert_eq!(instance_name(&id), hex(&id.0[..16]));
        assert_eq!(instance_name(&id).len(), 32);
        assert!(instance_name(&id).starts_with("abacad"));
        let entries = txt(&id, false);
        assert_eq!(entries, [("id".to_owned(), hex(&id.0))]);
        assert_eq!(entries[0].1.len(), 64);
        assert_eq!(parse_id(&entries[0].1), Some(id));
    }

    #[test]
    fn pairing_mode_open() {
        let id = id();
        assert!(txt(&id, true).contains(&("pair".to_owned(), "1".to_owned())));
        assert!(!txt(&id, false).iter().any(|(k, _)| k == "pair"));
        let s = sighting(Some(&hex(&id.0)), Some("1"), [Ipv4Addr::LOCALHOST], 1).unwrap();
        assert!(s.pairing);
        let s = sighting(Some(&hex(&id.0)), None, [Ipv4Addr::LOCALHOST], 1).unwrap();
        assert!(!s.pairing);
    }

    #[test]
    fn malformed_identity() {
        let good = hex(&id().0);
        assert!(parse_id(&good[..63]).is_none());
        assert!(parse_id(&format!("{good}0")).is_none());
        assert!(parse_id(&good.to_uppercase()).is_none());
        assert!(parse_id(&format!("{}g", &good[..63])).is_none());
        assert!(parse_id("é".repeat(32).as_str()).is_none());
        assert!(sighting(None, None, [Ipv4Addr::LOCALHOST], 1).is_none());
        assert!(sighting(Some(&good[..63]), None, [Ipv4Addr::LOCALHOST], 1).is_none());
    }
}
