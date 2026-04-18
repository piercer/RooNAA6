use std::net::Ipv4Addr;
use std::process::Command;

use crate::ts;

fn validate_host(host: &str) -> Result<(), String> {
    host.parse::<Ipv4Addr>()
        .map(|_| ())
        .map_err(|_| format!("invalid IPv4 address: {}", host))
}

fn run_iptables(verb: &str, naa_host: &str) -> Result<std::process::Output, String> {
    Command::new("/usr/sbin/iptables")
        .args(["-t", "nat", verb, "PREROUTING"])
        .args(["-s", naa_host])
        .args(["-p", "tcp", "--dport", "4321"])
        .args(["-j", "REDIRECT", "--to-port", "14321"])
        .output()
        .map_err(|e| format!("failed to run iptables: {}", e))
}

pub fn check_rule(naa_host: &str) -> bool {
    run_iptables("-C", naa_host)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn add_rule(naa_host: &str) -> Result<(), String> {
    validate_host(naa_host)?;
    if check_rule(naa_host) {
        eprintln!("{} [iptables] rule already present for {}", ts(), naa_host);
        return Ok(());
    }
    eprintln!("{} [iptables] adding rule for {}", ts(), naa_host);
    let output = run_iptables("-A", naa_host)?;
    if output.status.success() {
        eprintln!("{} [iptables] rule added for {}", ts(), naa_host);
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("iptables exited {}: {}", output.status, stderr.trim()))
    }
}

pub fn remove_rule(naa_host: &str) -> Result<(), String> {
    validate_host(naa_host)?;
    if !check_rule(naa_host) {
        eprintln!("{} [iptables] rule not present for {}, nothing to remove", ts(), naa_host);
        return Ok(());
    }
    eprintln!("{} [iptables] removing rule for {}", ts(), naa_host);
    let output = run_iptables("-D", naa_host)?;
    if output.status.success() {
        eprintln!("{} [iptables] rule removed for {}", ts(), naa_host);
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("iptables exited {}: {}", output.status, stderr.trim()))
    }
}
