mod config;
mod metadata;
mod discovery;
mod frame;
mod proxy;
mod roon;

use std::time::SystemTime;

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
    let cfg = config::load();

    eprintln!("{} RooNAA6 starting", ts());

    let mcast_iface = cfg.naa.mcast_iface;
    std::thread::Builder::new()
        .name("discovery".into())
        .spawn(move || discovery::run(mcast_iface))
        .unwrap();

    let shared = metadata::SharedMetadata::new();

    let roon_host = cfg.roon.host.clone();
    let roon_port = cfg.roon.port;
    let zone_name = cfg.roon.zone.clone();
    let token_file = cfg.roon.token_file.clone();
    let shared_roon = shared.clone();
    std::thread::Builder::new()
        .name("roon".into())
        .spawn(move || roon::run(shared_roon, &roon_host, roon_port, &zone_name, &token_file))
        .unwrap();

    let naa_host = cfg.naa.host;
    let naa_port = cfg.naa.port;

    let listener = std::net::TcpListener::bind(("0.0.0.0", naa_port)).unwrap();
    eprintln!(
        "{} NAA proxy: :{} -> {}:{}",
        ts(),
        naa_port,
        naa_host,
        naa_port
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

        let naa = match std::net::TcpStream::connect((&*naa_host, naa_port)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{} NAA connect failed: {}", ts(), e);
                continue;
            }
        };

        let client_r = client.try_clone().unwrap();
        let naa_r = naa.try_clone().unwrap();
        let client_w = client.try_clone().unwrap();
        let naa_w = naa.try_clone().unwrap();
        let shared_clone = shared.clone();

        let t1 = std::thread::Builder::new()
            .name("hqp-to-naa".into())
            .spawn(move || proxy::forward_hqp_to_naa(client_r, naa_w, shared_clone))
            .unwrap();

        let t2 = std::thread::Builder::new()
            .name("naa-to-hqp".into())
            .spawn(move || proxy::forward_passthrough(naa_r, client_w, "NAA->HQP"))
            .unwrap();

        t1.join().unwrap();
        let _ = client.shutdown(std::net::Shutdown::Both);
        let _ = naa.shutdown(std::net::Shutdown::Both);
        t2.join().unwrap();
        eprintln!("{} Session ended", ts());
    }
}
