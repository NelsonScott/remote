//! SSDP discovery of Roku devices (`M-SEARCH` for `roku:ecp`).

use std::collections::HashSet;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

pub const SSDP_MULTICAST: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 1900);
pub const SEARCH_TARGET: &str = "roku:ecp";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdpDevice {
    /// e.g. `http://192.168.1.221:8060/`
    pub location: String,
    /// e.g. `uuid:roku:ecp:YH00AB123456`
    pub usn: String,
    /// `ip:port` extracted from `location`, ready for `ecp::Client::new`.
    pub host: String,
}

impl SsdpDevice {
    /// Serial number from the USN, if it has the usual Roku shape.
    pub fn serial(&self) -> Option<&str> {
        self.usn.rsplit(':').next().filter(|s| !s.is_empty())
    }
}

pub fn m_search_message(mx_seconds: u8) -> String {
    format!(
        "M-SEARCH * HTTP/1.1\r\n\
         Host: 239.255.255.250:1900\r\n\
         Man: \"ssdp:discover\"\r\n\
         MX: {mx_seconds}\r\n\
         ST: {SEARCH_TARGET}\r\n\r\n"
    )
}

/// Parses one SSDP unicast reply. Returns `None` unless it is a 200 with a
/// LOCATION and an ST/USN that mentions `roku:ecp`.
pub fn parse_response(text: &str) -> Option<SsdpDevice> {
    let mut lines = text.split("\r\n");
    let status = lines.next()?.trim();
    if !status.starts_with("HTTP/1.1 200") && !status.starts_with("HTTP/1.0 200") {
        return None;
    }
    let mut location = None;
    let mut usn = None;
    let mut st = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "location" => location = Some(value.to_string()),
            "usn" => usn = Some(value.to_string()),
            "st" => st = Some(value.to_string()),
            _ => {}
        }
    }
    let location = location?;
    let usn = usn.unwrap_or_default();
    let is_roku =
        st.as_deref().is_some_and(|s| s.contains(SEARCH_TARGET)) || usn.contains(SEARCH_TARGET);
    if !is_roku {
        return None;
    }
    let host = host_from_location(&location)?;
    Some(SsdpDevice {
        location,
        usn,
        host,
    })
}

/// `http://192.168.1.221:8060/` -> `192.168.1.221:8060`
pub fn host_from_location(location: &str) -> Option<String> {
    let rest = location
        .strip_prefix("http://")
        .or_else(|| location.strip_prefix("https://"))?;
    let authority = rest.split('/').next()?;
    if authority.is_empty() {
        return None;
    }
    if authority.contains(':') {
        Some(authority.to_string())
    } else {
        Some(format!("{authority}:{}", crate::ecp::ECP_PORT))
    }
}

/// Multicast M-SEARCH on the default interface, collecting replies for
/// `timeout`. Devices are deduplicated by USN.
///
/// If nobody answers, falls back to a unicast M-SEARCH to every host of the
/// local /24. Host firewalls that only allow replies to traffic we started
/// (Fedora with the high UDP ports closed, most default firewalld zones on
/// servers) drop the answers to a multicast query, because connection
/// tracking cannot match a reply from 192.168.1.221 to a packet sent to
/// 239.255.255.250. Answers to a unicast query do match and get through.
pub fn search(timeout: Duration) -> io::Result<Vec<SsdpDevice>> {
    let found = search_at(SocketAddr::V4(SSDP_MULTICAST), timeout)?;
    if !found.is_empty() {
        return Ok(found);
    }
    let Some(local) = local_ipv4() else {
        return Ok(found);
    };
    let targets = sweep_targets(local);
    if targets.is_empty() {
        return Ok(found);
    }
    log::info!(
        "SSDP: no multicast replies, trying unicast to {} hosts near {local}",
        targets.len()
    );
    search_targets(&targets, timeout)
}

/// The IPv4 address the kernel would use to reach the SSDP group, i.e. the
/// LAN address. `connect` on UDP only picks a route; nothing is sent.
fn local_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(SSDP_MULTICAST).ok()?;
    match socket.local_addr().ok()? {
        SocketAddr::V4(a) => Some(*a.ip()),
        SocketAddr::V6(_) => None,
    }
}

/// Every other host in `local`'s /24, port 1900. Empty unless `local` is a
/// private (RFC 1918) address, so we never sweep a public network.
pub fn sweep_targets(local: Ipv4Addr) -> Vec<SocketAddr> {
    if !local.is_private() {
        return Vec::new();
    }
    let [a, b, c, me] = local.octets();
    (1..=254u8)
        .filter(|&d| d != me)
        .map(|d| SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(a, b, c, d), 1900)))
        .collect()
}

/// Same as [`search`] but with an explicit destination and no fallback
/// (tests use a loopback responder).
pub fn search_at(target: SocketAddr, timeout: Duration) -> io::Result<Vec<SsdpDevice>> {
    search_targets(&[target], timeout)
}

/// Sends the M-SEARCH to every target (once, then again halfway through)
/// and collects Roku replies until `timeout`.
pub fn search_targets(targets: &[SocketAddr], timeout: Duration) -> io::Result<Vec<SsdpDevice>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    if targets.iter().any(|t| t.ip().is_multicast()) {
        socket.set_multicast_ttl_v4(2)?;
    }
    let msg = m_search_message(timeout.as_secs().clamp(1, 5) as u8);
    let send_all = || {
        let mut sent = 0;
        for target in targets {
            // one unreachable neighbour must not abort the sweep
            if socket.send_to(msg.as_bytes(), target).is_ok() {
                sent += 1;
            }
        }
        sent
    };
    if send_all() == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NetworkUnreachable,
            "could not send M-SEARCH",
        ));
    }

    let start = Instant::now();
    let mut resent = false;
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    let mut buf = [0u8; 2048];
    while start.elapsed() < timeout {
        // one retry halfway through, SSDP is lossy
        if !resent && start.elapsed() > timeout / 2 {
            resent = true;
            send_all();
        }
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                let text = String::from_utf8_lossy(&buf[..n]);
                if let Some(mut dev) = parse_response(&text) {
                    // some firmwares answer with 0.0.0.0; trust the sender
                    if dev.host.starts_with("0.0.0.0") {
                        dev.host = format!("{}:{}", from.ip(), crate::ecp::ECP_PORT);
                    }
                    let key = if dev.usn.is_empty() {
                        dev.host.clone()
                    } else {
                        dev.usn.clone()
                    };
                    if seen.insert(key) {
                        log::info!("SSDP: {} at {}", dev.usn, dev.host);
                        found.push(dev);
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::ConnectionRefused
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::HostUnreachable
                        | io::ErrorKind::NetworkUnreachable
                ) =>
            {
                continue
            }
            Err(e) => return Err(e),
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = "HTTP/1.1 200 OK\r\nCache-Control: max-age=3600\r\nST: roku:ecp\r\nUSN: uuid:roku:ecp:YH00AB123456\r\nExt: \r\nServer: Roku/15.3.4 UPnP/1.0 Roku/15.3.4\r\nLOCATION: http://192.168.1.221:8060/\r\ndevice-group.roku.com: 0A1B2C\r\n\r\n";

    #[test]
    fn parses_roku_reply() {
        let d = parse_response(REPLY).unwrap();
        assert_eq!(d.host, "192.168.1.221:8060");
        assert_eq!(d.location, "http://192.168.1.221:8060/");
        assert_eq!(d.serial(), Some("YH00AB123456"));
    }

    #[test]
    fn ignores_non_roku_and_bad_replies() {
        assert!(parse_response(
            "HTTP/1.1 200 OK\r\nST: upnp:rootdevice\r\nLOCATION: http://x/\r\n\r\n"
        )
        .is_none());
        assert!(parse_response("NOTIFY * HTTP/1.1\r\nST: roku:ecp\r\n\r\n").is_none());
        assert!(parse_response("HTTP/1.1 200 OK\r\nST: roku:ecp\r\n\r\n").is_none());
    }

    #[test]
    fn host_from_location_forms() {
        assert_eq!(
            host_from_location("http://10.0.0.5:8060/").as_deref(),
            Some("10.0.0.5:8060")
        );
        assert_eq!(
            host_from_location("http://10.0.0.5/").as_deref(),
            Some("10.0.0.5:8060")
        );
        assert_eq!(host_from_location("ftp://x"), None);
    }

    #[test]
    fn m_search_has_required_headers() {
        let m = m_search_message(2);
        assert!(m.starts_with("M-SEARCH * HTTP/1.1\r\n"));
        assert!(m.contains("ST: roku:ecp\r\n"));
        assert!(m.contains("Man: \"ssdp:discover\"\r\n"));
        assert!(m.ends_with("\r\n\r\n"));
    }

    /// A loopback UDP responder that answers like a Roku, twice (to test
    /// dedupe), plus a non-Roku answer that must be ignored.
    #[test]
    fn search_at_dedupes_by_usn() {
        let responder = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = responder.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            let (n, from) = responder.recv_from(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(req.contains("ST: roku:ecp"));
            responder.send_to(REPLY.as_bytes(), from).unwrap();
            responder.send_to(REPLY.as_bytes(), from).unwrap();
            responder
                .send_to(
                    b"HTTP/1.1 200 OK\r\nST: upnp:rootdevice\r\nUSN: uuid:printer\r\nLOCATION: http://192.168.1.9:80/\r\n\r\n",
                    from,
                )
                .unwrap();
        });
        let found = search_at(addr, Duration::from_millis(600)).unwrap();
        t.join().unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].host, "192.168.1.221:8060");
    }

    #[test]
    fn sweep_covers_the_slash_24_except_us() {
        let t = sweep_targets(Ipv4Addr::new(192, 168, 1, 66));
        assert_eq!(t.len(), 253);
        assert!(t.contains(&"192.168.1.221:1900".parse().unwrap()));
        assert!(t.contains(&"192.168.1.1:1900".parse().unwrap()));
        assert!(!t.contains(&"192.168.1.66:1900".parse().unwrap()));
        assert!(!t.contains(&"192.168.1.255:1900".parse().unwrap()));
        assert!(sweep_targets(Ipv4Addr::new(8, 8, 8, 8)).is_empty());
        assert!(sweep_targets(Ipv4Addr::new(127, 0, 0, 1)).is_empty());
    }

    /// The unicast sweep finds a responder among silent targets.
    #[test]
    fn search_targets_finds_the_one_that_answers() {
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        let responder = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = responder.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            let (_, from) = responder.recv_from(&mut buf).unwrap();
            responder.send_to(REPLY.as_bytes(), from).unwrap();
        });
        let targets = [silent.local_addr().unwrap(), addr];
        let found = search_targets(&targets, Duration::from_millis(600)).unwrap();
        t.join().unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].serial(), Some("YH00AB123456"));
    }
}
