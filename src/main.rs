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

    eprintln!(
        "{} NAA proxy: :{} -> {}:{}",
        ts(),
        NAA_PORT,
        NAA_HOST,
        NAA_PORT
    );

    // TCP listener will go here in Task 4
    loop {
        thread::park();
    }
}
