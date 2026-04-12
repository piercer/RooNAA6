mod config;
mod discovery;
mod frame;
mod metadata;
mod proxy;
mod roon;
#[cfg(test)]
mod tests;

use std::time::SystemTime;

pub fn ts() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let secs = now.as_secs() % 86400;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60,
        now.subsec_millis(),
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

    let roon_cfg = cfg.roon;
    let shared_roon = shared.clone();
    std::thread::Builder::new()
        .name("roon".into())
        .spawn(move || roon::run(shared_roon, &roon_cfg))
        .unwrap();

    let naa_host = cfg.naa.host;
    let naa_port = discovery::NAA_PORT;

    let listener = std::net::TcpListener::bind(("0.0.0.0", naa_port)).unwrap();
    eprintln!(
        "{} NAA proxy: :{} -> {}:{}",
        ts(),
        naa_port,
        naa_host,
        naa_port,
    );

    for stream in listener.incoming() {
        let client = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{} accept error: {}", ts(), e);
                continue;
            }
        };
        let addr = client
            .peer_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "unknown".into());
        eprintln!("{} HQP connected from {}", ts(), addr);

        let naa = match std::net::TcpStream::connect((&*naa_host, naa_port)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{} NAA connect failed: {}", ts(), e);
                continue;
            }
        };

        client.set_nodelay(true).ok();
        naa.set_nodelay(true).ok();

        let client_r = client.try_clone().unwrap();
        let naa_r = naa.try_clone().unwrap();
        let client_w = client.try_clone().unwrap();
        let naa_w = naa.try_clone().unwrap();
        let shared_clone = shared.clone();

        // First thread to finish triggers teardown of both sockets,
        // preventing a deadlock when one side disconnects while the other is idle.
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let done_tx2 = done_tx.clone();

        let t1 = std::thread::Builder::new()
            .name("hqp-to-naa".into())
            .spawn(move || {
                proxy::forward_hqp_to_naa(client_r, naa_w, shared_clone);
                let _ = done_tx.send(());
            })
            .unwrap();

        let t2 = std::thread::Builder::new()
            .name("naa-to-hqp".into())
            .spawn(move || {
                proxy::forward_passthrough(naa_r, client_w, "NAA->HQP");
                let _ = done_tx2.send(());
            })
            .unwrap();

        let _ = done_rx.recv();
        let _ = client.shutdown(std::net::Shutdown::Both);
        let _ = naa.shutdown(std::net::Shutdown::Both);

        if let Err(e) = t1.join() {
            eprintln!("{} hqp-to-naa panicked: {:?}", ts(), e);
        }
        if let Err(e) = t2.join() {
            eprintln!("{} naa-to-hqp panicked: {:?}", ts(), e);
        }
        eprintln!("{} Session ended", ts());
    }
}
