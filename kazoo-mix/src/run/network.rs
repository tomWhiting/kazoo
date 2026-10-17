use std::net::{Ipv4Addr, SocketAddr};

use nix::ifaddrs::getifaddrs;
use nix::net::if_::InterfaceFlags;

/// The status note, after where the desk listens on the network, if it
/// does.
pub(super) fn note(note: String, listening: Option<&str>) -> String {
    match listening {
        Some(listening) => format!("{listening}; {note}"),
        None => note,
    }
}

/// Where the desk listens, with this machine's local IPv4 addresses, so
/// the `host:port` to give `kzoo --desk` on another machine can be read
/// off the desk. Read once at startup: it asks the system for its
/// interfaces.
pub(super) fn listening(address: SocketAddr) -> String {
    describe(
        address,
        local_ipv4s().map_err(|error| format!("could not be read: {error}")),
    )
}

/// `address`, then the machine's `addresses`, or why they are not known.
fn describe(address: SocketAddr, addresses: Result<Vec<Ipv4Addr>, String>) -> String {
    let mut line = format!("listening on {address} ");
    match addresses {
        Ok(addresses) if addresses.is_empty() => line.push_str("(this Mac has no network address)"),
        Ok(addresses) => {
            let addresses: Vec<String> = addresses.iter().map(ToString::to_string).collect();
            line.push_str("(this Mac: ");
            line.push_str(&addresses.join(", "));
            line.push(')');
        }
        Err(why) => {
            line.push_str("(this Mac's addresses ");
            line.push_str(&why);
            line.push(')');
        }
    }
    line
}

/// This machine's IPv4 addresses on interfaces that are up, loopback
/// left out, each once, in the system's order.
fn local_ipv4s() -> nix::Result<Vec<Ipv4Addr>> {
    let mut addresses = Vec::new();
    for interface in getifaddrs()? {
        if !interface.flags.contains(InterfaceFlags::IFF_UP) {
            continue;
        }
        let Some(ip) = interface
            .address
            .as_ref()
            .and_then(|address| address.as_sockaddr_in())
            .map(nix::sys::socket::SockaddrIn::ip)
        else {
            continue;
        };
        if !ip.is_loopback() && !addresses.contains(&ip) {
            addresses.push(ip);
        }
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn listening_address_is_visible_in_the_header_and_absent_by_default() {
        assert_eq!(note("ready".to_owned(), None), "ready");
        let address = "0.0.0.0:7700".parse().unwrap();
        let listening = listening(address);
        let status = crate::ui::StatusInfo {
            sample_rate: 48_000,
            channels: 2,
            buffer_frames: Some(256),
            note: note("ready".to_owned(), Some(&listening)),
            note_alert: false,
            stream_errors: 0,
            engine_faults: 0,
            mouse: true,
            hub_priority: crate::ui::HubPriority::RealTime,
        };
        let mut terminal = Terminal::new(TestBackend::new(200, 34)).unwrap();
        let mut desk = crate::desk::Desk::new();
        let shared = crate::shared::SharedState::new();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut desk, &shared, &status))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            text.contains("listening on 0.0.0.0:7700 (this Mac"),
            "{text}"
        );
        assert!(text.contains("; ready"), "{text}");
    }

    #[test]
    fn the_listening_line_names_this_macs_addresses() {
        let address = "0.0.0.0:7700".parse().unwrap();
        let house = vec![Ipv4Addr::new(192, 168, 1, 5), Ipv4Addr::new(100, 64, 0, 7)];
        assert_eq!(
            describe(address, Ok(house)),
            "listening on 0.0.0.0:7700 (this Mac: 192.168.1.5, 100.64.0.7)"
        );
        assert_eq!(
            describe(address, Ok(Vec::new())),
            "listening on 0.0.0.0:7700 (this Mac has no network address)"
        );
        assert_eq!(
            describe(address, Err("could not be read: EMFILE".to_owned())),
            "listening on 0.0.0.0:7700 (this Mac's addresses could not be read: EMFILE)"
        );
        let v6 = "[::]:7700".parse().unwrap();
        assert!(describe(v6, Ok(Vec::new())).starts_with("listening on [::]:7700 "));
    }

    #[test]
    fn this_macs_addresses_are_read_without_loopback() {
        let addresses = local_ipv4s().unwrap();
        assert!(
            addresses.iter().all(|ip| !ip.is_loopback()),
            "{addresses:?}"
        );
        let mut unique = addresses.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), addresses.len(), "{addresses:?}");
    }
}
