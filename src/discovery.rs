use std::net::{Ipv4Addr, UdpSocket};

use crate::{ts, NAA_PORT};

const MCAST_ADDRS: [Ipv4Addr; 2] = [
    Ipv4Addr::new(224, 0, 0, 199),
    Ipv4Addr::new(239, 192, 0, 199),
];

const DISCOVER_RESPONSE: &[u8] = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<networkaudio>\
<discover result=\"OK\" name=\"RooNAA6 Proxy\" version=\"eversolo naa\" protocol=\"6\" trigger=\"0\">\
network audio\
</discover>\
</networkaudio>\n";

pub fn run(bind_addr: Ipv4Addr) {
    let sock = UdpSocket::bind(("0.0.0.0", NAA_PORT)).unwrap();

    for mcast in &MCAST_ADDRS {
        if let Err(e) = sock.join_multicast_v4(mcast, &bind_addr) {
            eprintln!("{} [discovery] failed to join {}: {}", ts(), mcast, e);
        }
    }

    eprintln!(
        "{} [discovery] listening on :{} (mcast on {})",
        ts(),
        NAA_PORT,
        bind_addr
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
                    let _ = sock.send_to(DISCOVER_RESPONSE, &addr);
                }
            }
            Err(e) => {
                eprintln!("{} [discovery] recv error: {}", ts(), e);
            }
        }
    }
}
