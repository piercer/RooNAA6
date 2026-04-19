use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use crate::frame::extract_xml_attr;
use crate::ts;

pub const NAA_PORT: u16 = 43210;

const MCAST_ADDRS: [Ipv4Addr; 2] = [
    Ipv4Addr::new(224, 0, 0, 199),
    Ipv4Addr::new(239, 192, 0, 199),
];

const PROXY_NAME: &str = "RooNAA6 Proxy";

#[derive(Debug, Clone)]
pub struct NaaEndpoint {
    pub name: String,
    pub version: String,
    pub protocol: String,
    pub trigger: String,
    pub addr: SocketAddr,
}

/// Send multicast discover queries and collect NAA endpoint responses.
/// Must be called BEFORE the responder thread starts (to avoid self-discovery).
pub fn discover_endpoints(bind_addr: Ipv4Addr) -> Vec<NaaEndpoint> {
    let query = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<networkaudio><discover>network audio</discover></networkaudio>\n";

    let sock = UdpSocket::bind((bind_addr, 0)).expect("bind discovery client socket");
    sock.set_read_timeout(Some(Duration::from_secs(3))).unwrap();

    for mcast in &MCAST_ADDRS {
        let _ = sock.send_to(query, (*mcast, NAA_PORT));
    }
    eprintln!("{} [discovery] scanning for NAA endpoints...", ts());

    let mut endpoints = Vec::new();
    let mut buf = [0u8; 4096];
    while let Ok((len, addr)) = sock.recv_from(&mut buf) {
        if let Some(ep) = parse_discover_response(&buf[..len], addr) {
            if ep.name == PROXY_NAME {
                continue;
            }
            if endpoints.iter().any(|e: &NaaEndpoint| e.addr.ip() == ep.addr.ip()) {
                continue;
            }
            eprintln!(
                "{} [discovery] found: {} (version={:?}, protocol={}, addr={})",
                ts(), ep.name, ep.version, ep.protocol, ep.addr,
            );
            endpoints.push(ep);
        }
    }

    eprintln!("{} [discovery] found {} endpoint(s)", ts(), endpoints.len());
    endpoints
}

pub(crate) fn parse_discover_response(data: &[u8], addr: SocketAddr) -> Option<NaaEndpoint> {
    let text = std::str::from_utf8(data).ok()?;
    let discover_start = text.find("<discover ")?;
    let discover = &text[discover_start..];
    if !discover.contains("result=\"OK\"") {
        return None;
    }
    let name = extract_xml_attr(discover, "name")?;
    let version = extract_xml_attr(discover, "version")?;
    let protocol = extract_xml_attr(discover, "protocol").unwrap_or_else(|| "6".to_string());
    let trigger = extract_xml_attr(discover, "trigger").unwrap_or_else(|| "0".to_string());

    Some(NaaEndpoint {
        name,
        version,
        protocol,
        trigger,
        addr: SocketAddr::new(addr.ip(), NAA_PORT),
    })
}

fn build_discover_response(ep: &NaaEndpoint) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<networkaudio>\
<discover result=\"OK\" name=\"{PROXY_NAME}\" version=\"{}\" protocol=\"{}\" trigger=\"{}\">\
network audio\
</discover>\
</networkaudio>\n",
        ep.version, ep.protocol, ep.trigger,
    )
    .into_bytes()
}

pub fn run(bind_addr: Ipv4Addr, endpoint: NaaEndpoint) {
    let response = build_discover_response(&endpoint);
    let sock = UdpSocket::bind(("0.0.0.0", NAA_PORT)).unwrap();

    for mcast in &MCAST_ADDRS {
        if let Err(e) = sock.join_multicast_v4(mcast, &bind_addr) {
            eprintln!("{} [discovery] failed to join {}: {}", ts(), mcast, e);
        }
    }

    eprintln!(
        "{} [discovery] listening on :{} (mcast on {}, mimicking {:?})",
        ts(),
        NAA_PORT,
        bind_addr,
        endpoint.name,
    );

    let mut buf = [0u8; 4096];
    loop {
        match sock.recv_from(&mut buf) {
            Ok((len, addr)) => {
                let data = &buf[..len];
                if data.windows(8).any(|w| w == b"discover")
                    && data.windows(13).any(|w| w == b"network audio")
                {
                    eprintln!("{} [discovery] responded to {}", ts(), addr);
                    let _ = sock.send_to(&response, addr);
                }
            }
            Err(e) => {
                eprintln!("{} [discovery] recv error: {}", ts(), e);
            }
        }
    }
}
