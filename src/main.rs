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
    let naa_version = cfg.naa.version.clone();
    std::thread::Builder::new()
        .name("discovery".into())
        .spawn(move || discovery::run(mcast_iface, naa_version))
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

    // Status proxy: intercepts HQPlayer's control channel (port 4321)
    // so we can rewrite song="Roon" with the actual track title.
    // Requires iptables redirect: -t nat -A PREROUTING -s <naa_host> -p tcp --dport 4321 -j REDIRECT --to-port 14321
    let status_shared = shared.clone();
    std::thread::Builder::new()
        .name("status-proxy".into())
        .spawn(move || {
            const STATUS_LISTEN_PORT: u16 = 14321;
            const HQP_CONTROL_PORT: u16 = 4321;
            let listener = match std::net::TcpListener::bind(("0.0.0.0", STATUS_LISTEN_PORT)) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("{} Status proxy bind failed on :{}: {}", ts(), STATUS_LISTEN_PORT, e);
                    return;
                }
            };
            eprintln!("{} Status proxy: :{} -> localhost:{}", ts(), STATUS_LISTEN_PORT, HQP_CONTROL_PORT);

            for stream in listener.incoming() {
                let naa_conn = match stream {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("{} status accept error: {}", ts(), e);
                        continue;
                    }
                };
                let addr = naa_conn.peer_addr().map(|a| a.to_string()).unwrap_or_else(|_| "unknown".into());
                eprintln!("{} Status client connected from {}", ts(), addr);

                let hqp_conn = match std::net::TcpStream::connect(("127.0.0.1", HQP_CONTROL_PORT)) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("{} Status HQP connect failed: {}", ts(), e);
                        continue;
                    }
                };

                naa_conn.set_nodelay(true).ok();
                hqp_conn.set_nodelay(true).ok();

                let hqp_r = hqp_conn.try_clone().unwrap();
                let naa_w = naa_conn.try_clone().unwrap();
                let naa_r = naa_conn.try_clone().unwrap();
                let hqp_w = hqp_conn.try_clone().unwrap();
                let ss = status_shared.clone();

                let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
                let done_tx2 = done_tx.clone();

                std::thread::Builder::new()
                    .name("status-hqp-to-naa".into())
                    .spawn(move || {
                        proxy::forward_status_to_naa(hqp_r, naa_w, ss);
                        let _ = done_tx.send(());
                    })
                    .unwrap();

                std::thread::Builder::new()
                    .name("status-naa-to-hqp".into())
                    .spawn(move || {
                        proxy::forward_passthrough(naa_r, hqp_w, "Status<-NAA");
                        let _ = done_tx2.send(());
                    })
                    .unwrap();

                let _ = done_rx.recv();
                let _ = naa_conn.shutdown(std::net::Shutdown::Both);
                let _ = hqp_conn.shutdown(std::net::Shutdown::Both);
                eprintln!("{} Status session ended", ts());
            }
        })
        .unwrap();

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
