mod metadata;
mod discovery;
mod frame;
mod proxy;

use std::time::SystemTime;

pub const NAA_HOST: &str = "192.168.30.109";
pub const NAA_PORT: u16 = 43210;

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
}
