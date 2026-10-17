use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::net::Ipv6Addr;
use std::path::PathBuf;

use super::HubAddress;

impl HubAddress {
    /// Parse a socket path containing `/`, or a host and port.
    pub fn parse(text: &str) -> io::Result<Self> {
        let invalid = |reason| io::Error::new(io::ErrorKind::InvalidInput, reason);
        if text.contains('\0') {
            return Err(invalid("desk address contains NUL"));
        }
        if text.contains('/') {
            return Ok(Self::Socket(PathBuf::from(text)));
        }
        let (host, port) = text.rsplit_once(':').ok_or_else(|| {
            invalid("desk address must be host:port or a socket path containing /")
        })?;
        if port.parse::<u16>().map_or(true, |port| port == 0)
            || !port.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(invalid("desk port must be a number from 1 to 65535"));
        }
        let valid_host = host.strip_prefix('[').map_or_else(
            || {
                !host.is_empty()
                    && host.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')
                    })
            },
            |host| host.strip_suffix(']').is_some_and(is_ipv6_host),
        );
        if !valid_host {
            return Err(invalid(
                "desk host must be a hostname, IPv4, or IPv6 in brackets (with its interface after %, if link-local)",
            ));
        }
        Ok(Self::Tcp(text.to_owned()))
    }

    pub(crate) fn resolve(&self) -> io::Result<Self> {
        self.resolve_with(
            std::env::var_os("KAZOO_DESK").as_deref(),
            super::super::discovery::hub_socket,
        )
    }

    fn resolve_with(
        &self,
        desk: Option<&OsStr>,
        local: impl FnOnce() -> io::Result<PathBuf>,
    ) -> io::Result<Self> {
        if !matches!(self, Self::Discover) {
            return Ok(self.clone());
        }
        desk.map_or_else(
            || local().map(Self::Socket),
            |value| {
                value
                    .to_str()
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "KAZOO_DESK is not valid UTF-8")
                    })
                    .and_then(Self::parse)
                    .map_err(|error| {
                        io::Error::new(error.kind(), format!("invalid KAZOO_DESK: {error}"))
                    })
            },
        )
    }
}

/// An IPv6 address, with the interface a link-local one is reached on
/// after `%` (`fe80::1%en0`): 1 to 15 letters, digits, `.`, `_` or `-`.
fn is_ipv6_host(host: &str) -> bool {
    let (address, zone) = host
        .split_once('%')
        .map_or((host, None), |(a, z)| (a, Some(z)));
    address.parse::<Ipv6Addr>().is_ok()
        && zone.is_none_or(|zone| {
            (1..=15).contains(&zone.len())
                && zone
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}

/// A parsed `host:port`'s host, without an IPv6 address's brackets, and its
/// port: what the resolver is asked for.
pub fn host_and_port(target: &str) -> io::Result<(&str, u16)> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("desk address {target} is not host:port"),
        )
    };
    let (host, port) = target.rsplit_once(':').ok_or_else(invalid)?;
    let port = port.parse::<u16>().map_err(|_| invalid())?;
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    Ok((host, port))
}

impl fmt::Display for HubAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Discover => formatter.write_str("discovered desk"),
            Self::Socket(path) => path.display().fmt(formatter),
            Self::Tcp(address) => formatter.write_str(address),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::HubAddress;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    #[test]
    fn desk_addresses_parse_tcp_and_socket_forms() {
        for text in [
            "tom-old.local:7700",
            "127.0.0.1:7700",
            "[::1]:7700",
            "[fe80::1%en0]:7700",
        ] {
            assert!(
                matches!(HubAddress::parse(text).unwrap(), HubAddress::Tcp(value) if value == text)
            );
        }
        for text in ["/tmp/desk.sock", "./desk.sock", "relative/desk.sock"] {
            assert!(
                matches!(HubAddress::parse(text).unwrap(), HubAddress::Socket(value) if value == std::path::Path::new(text))
            );
        }
    }

    #[test]
    fn desk_addresses_refuse_missing_or_invalid_host_and_port() {
        for text in [
            "",
            "desk.sock",
            "localhost",
            ":7700",
            "host:",
            "host:no",
            "host:65536",
            "::1:7700",
            "[no]:7700",
            "host name:7700",
            "host:0",
            "[fe80::1%]:7700",
            "[fe80::1%en 0]:7700",
            "[fe80::1%abcdefghijklmnop]:7700",
            "/tmp/a\0b",
        ] {
            let error = HubAddress::parse(text).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
            assert!(!error.to_string().is_empty());
        }
    }

    #[test]
    fn discover_uses_desk_environment_without_local_fallback() {
        for text in ["desk.local:7700", "/tmp/remote.sock"] {
            let address = HubAddress::Discover
                .resolve_with(Some(OsStr::new(text)), || {
                    panic!("local discovery must not run")
                })
                .unwrap();
            assert_eq!(
                format!("{address:?}"),
                format!("{:?}", HubAddress::parse(text).unwrap())
            );
        }
        let error = HubAddress::Discover
            .resolve_with(Some(OsStr::new("bad-desk")), || {
                panic!("bad environment must not fall back")
            })
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("KAZOO_DESK"));
        let address = HubAddress::Discover
            .resolve_with(None, || Ok(PathBuf::from("/tmp/local.sock")))
            .unwrap();
        assert!(
            matches!(address, HubAddress::Socket(path) if path == std::path::Path::new("/tmp/local.sock"))
        );
        let explicit = HubAddress::Socket(PathBuf::from("/tmp/explicit.sock"));
        assert!(matches!(
            explicit
                .resolve_with(Some(OsStr::new("bad")), || panic!(
                    "explicit address must win"
                ))
                .unwrap(),
            HubAddress::Socket(_)
        ));
    }
}
