use std::net::{Ipv4Addr, UdpSocket};

use crate::ts;

pub const NAA_PORT: u16 = 43210;

const MCAST_ADDRS: [Ipv4Addr; 2] = [
    Ipv4Addr::new(224, 0, 0, 199),
    Ipv4Addr::new(239, 192, 0, 199),
];

/// Build the discover response XML with a configurable version string.
/// The `version` field is an opaque identifier that HQPlayer keys off to
/// decide which NAA dialect to speak — it must match whatever the real
/// DAC reports. Default is "eversolo naa".
fn build_discover_response(version: &str) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<networkaudio>\
<discover result=\"OK\" name=\"RooNAA6 Proxy\" version=\"{}\" protocol=\"6\" trigger=\"0\">\
network audio\
</discover>\
</networkaudio>\n",
        version,
    )
    .into_bytes()
}

pub fn run(bind_addr: Ipv4Addr, version: String) {
    let response = build_discover_response(&version);
    let sock = UdpSocket::bind(("0.0.0.0", NAA_PORT)).unwrap();

    for mcast in &MCAST_ADDRS {
        if let Err(e) = sock.join_multicast_v4(mcast, &bind_addr) {
            eprintln!("{} [discovery] failed to join {}: {}", ts(), mcast, e);
        }
    }

    eprintln!(
        "{} [discovery] listening on :{} (mcast on {}, version={:?})",
        ts(),
        NAA_PORT,
        bind_addr,
        version,
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
                    let _ = sock.send_to(&response, &addr);
                }
            }
            Err(e) => {
                eprintln!("{} [discovery] recv error: {}", ts(), e);
            }
        }
    }
}
