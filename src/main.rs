mod config;
mod discovery;
mod frame;
mod iptables;
mod metadata;
mod proxy;
mod roon;
#[cfg(test)]
mod tests;

use std::time::SystemTime;

use discovery::NaaEndpoint;

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

/// Resolve which NAA endpoint to proxy based on config and discovery results.
fn resolve_target(cfg: &config::NaaConfig, endpoints: &[NaaEndpoint]) -> NaaEndpoint {
    let fallback = |host: &str, version: Option<&String>| NaaEndpoint {
        name: host.to_string(),
        version: version.cloned().unwrap_or_else(|| "naa".to_string()),
        protocol: "6".to_string(),
        trigger: "0".to_string(),
        addr: format!("{host}:{}", discovery::NAA_PORT).parse().unwrap(),
    };

    // If target name is set, find it among discovered endpoints.
    if let Some(ref target) = cfg.target {
        if let Some(ep) = endpoints.iter().find(|e| e.name == *target) {
            let mut ep = ep.clone();
            if let Some(ref v) = cfg.version {
                ep.version = v.clone();
            }
            return ep;
        }
        eprintln!("ERROR: target {:?} not found among discovered endpoints:", target);
        for ep in endpoints {
            eprintln!("  - {} ({})", ep.name, ep.addr.ip());
        }
        if let Some(ref host) = cfg.host {
            eprintln!("Falling back to configured host={}", host);
            return fallback(host, cfg.version.as_ref());
        }
        std::process::exit(1);
    }

    // No target name — use explicit host if set.
    if let Some(ref host) = cfg.host {
        if let Some(ep) = endpoints.iter().find(|e| e.addr.ip().to_string() == *host) {
            let mut ep = ep.clone();
            if let Some(ref v) = cfg.version {
                ep.version = v.clone();
            }
            return ep;
        }
        return fallback(host, cfg.version.as_ref());
    }

    // No target, no host — use sole discovered endpoint.
    match endpoints.len() {
        0 => {
            eprintln!("ERROR: no NAA endpoints discovered and no host configured");
            std::process::exit(1);
        }
        1 => {
            let mut ep = endpoints[0].clone();
            eprintln!("{} auto-selected sole endpoint: {}", ts(), ep.name);
            if let Some(ref v) = cfg.version {
                ep.version = v.clone();
            }
            ep
        }
        _ => {
            eprintln!("ERROR: multiple NAA endpoints discovered — set [naa] target to select one:");
            for ep in endpoints {
                eprintln!("  - {:?} ({})", ep.name, ep.addr.ip());
            }
            std::process::exit(1);
        }
    }
}

fn main() {
    let cfg = config::load();

    eprintln!("{} RooNAA6 starting", ts());

    let mcast_iface = cfg.naa.mcast_iface;

    // Discover NAA endpoints BEFORE starting the responder (avoids self-discovery).
    let endpoints = discovery::discover_endpoints(mcast_iface);

    let target = resolve_target(&cfg.naa, &endpoints);
    let naa_host = target.addr.ip().to_string();
    eprintln!(
        "{} target: {} at {} (version={:?})",
        ts(), target.name, naa_host, target.version,
    );

    if let Some(ref ipt_cfg) = cfg.iptables {
        if ipt_cfg.enable {
            if let Err(e) = iptables::add_rule(&ipt_cfg.naa_host) {
                eprintln!("{} [iptables] failed to add rule: {}", ts(), e);
            }
        }
    }

    std::thread::Builder::new()
        .name("discovery".into())
        .spawn(move || discovery::run(mcast_iface, target))
        .unwrap();

    let shared = metadata::SharedMetadata::new();

    let roon_cfg = cfg.roon;
    let shared_roon = shared.clone();
    std::thread::Builder::new()
        .name("roon".into())
        .spawn(move || roon::run(shared_roon, &roon_cfg))
        .unwrap();

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
