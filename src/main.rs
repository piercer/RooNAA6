mod metadata;
mod discovery;
mod frame;
mod proxy;

use std::net::Ipv4Addr;
use std::thread;
use std::time::SystemTime;

pub const NAA_HOST: &str = "192.168.30.109";
pub const NAA_PORT: u16 = 43210;
pub const BIND_ADDR: Ipv4Addr = Ipv4Addr::new(0, 0, 0, 0);
// For multicast, set this to the interface IP where HQPlayer discovers NAA devices
pub const MCAST_IFACE: Ipv4Addr = Ipv4Addr::new(192, 168, 30, 212);

pub fn ts() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let secs = now.as_secs() % 86400;
    let millis = now.as_millis() % 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60,
        millis
    )
}

fn main() {
    eprintln!("{} RooNAA6 starting", ts());

    thread::Builder::new()
        .name("discovery".into())
        .spawn(move || discovery::run(MCAST_IFACE))
        .unwrap();

    let shared = metadata::SharedMetadata::new();

    // Hardcoded test metadata for Stage 1
    shared.set(metadata::Metadata {
        title: "Test Title".into(),
        artist: "Test Artist".into(),
        album: "Test Album".into(),
        ..Default::default()
    });

    let listener = std::net::TcpListener::bind(("0.0.0.0", NAA_PORT)).unwrap();
    eprintln!(
        "{} NAA proxy: :{} -> {}:{}",
        ts(),
        NAA_PORT,
        NAA_HOST,
        NAA_PORT
    );

    for stream in listener.incoming() {
        let client = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{} accept error: {}", ts(), e);
                continue;
            }
        };
        let addr = client.peer_addr().unwrap();
        eprintln!("{} HQP connected from {}", ts(), addr);

        let naa = match std::net::TcpStream::connect((NAA_HOST, NAA_PORT)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{} NAA connect failed: {}", ts(), e);
                continue;
            }
        };

        let client_r = client.try_clone().unwrap();
        let naa_r = naa.try_clone().unwrap();
        let client_w = client;
        let naa_w = naa;
        let shared_clone = shared.clone();

        let t1 = thread::Builder::new()
            .name("hqp-to-naa".into())
            .spawn(move || proxy::forward_hqp_to_naa(client_r, naa_w, shared_clone))
            .unwrap();

        let t2 = thread::Builder::new()
            .name("naa-to-hqp".into())
            .spawn(move || proxy::forward_passthrough(naa_r, client_w, "NAA->HQP"))
            .unwrap();

        t1.join().unwrap();
        t2.join().unwrap();
        eprintln!("{} Session ended", ts());
    }
}
