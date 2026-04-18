use std::net::Ipv4Addr;
use std::process::Command;

use crate::ts;

fn validate_host(host: &str) -> Result<(), String> {
    host.parse::<Ipv4Addr>()
        .map(|_| ())
        .map_err(|_| format!("invalid IPv4 address: {}", host))
}

/// The iptables rule arguments (without the -C/-A/-D verb) as a shared constant.
fn rule_args(naa_host: &str) -> Vec<String> {
    vec![
        "-t".into(), "nat".into(),
        "PREROUTING".into(),
        "-s".into(), naa_host.into(),
        "-p".into(), "tcp".into(),
        "--dport".into(), "4321".into(),
        "-j".into(), "REDIRECT".into(),
        "--to-port".into(), "14321".into(),
    ]
}

/// Returns true if the PREROUTING redirect rule already exists.
pub fn check_rule(naa_host: &str) -> bool {
    let mut args = vec!["-t".to_string(), "nat".to_string(), "-C".to_string()];
    args.extend(rule_args(naa_host));
    let status = Command::new("iptables")
        .args(&args)
        .status();
    match status {
        Ok(s) => s.success(),
        Err(_) => false,
    }
}

/// Adds the PREROUTING redirect rule if it is not already present (idempotent).
pub fn add_rule(naa_host: &str) -> Result<(), String> {
    validate_host(naa_host)?;
    if check_rule(naa_host) {
        eprintln!("{} [iptables] rule already present for {}", ts(), naa_host);
        return Ok(());
    }

    let mut args = vec!["-t".to_string(), "nat".to_string(), "-A".to_string()];
    args.extend(rule_args(naa_host));

    eprintln!("{} [iptables] adding rule for {}", ts(), naa_host);
    let output = Command::new("iptables")
        .args(&args)
        .output()
        .map_err(|e| format!("failed to run iptables: {}", e))?;

    if output.status.success() {
        eprintln!("{} [iptables] rule added for {}", ts(), naa_host);
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("iptables exited {}: {}", output.status, stderr.trim()))
    }
}

/// Removes the PREROUTING redirect rule if it is present (idempotent).
pub fn remove_rule(naa_host: &str) -> Result<(), String> {
    validate_host(naa_host)?;
    if !check_rule(naa_host) {
        eprintln!("{} [iptables] rule not present for {}, nothing to remove", ts(), naa_host);
        return Ok(());
    }

    let mut args = vec!["-t".to_string(), "nat".to_string(), "-D".to_string()];
    args.extend(rule_args(naa_host));

    eprintln!("{} [iptables] removing rule for {}", ts(), naa_host);
    let output = Command::new("iptables")
        .args(&args)
        .output()
        .map_err(|e| format!("failed to run iptables: {}", e))?;

    if output.status.success() {
        eprintln!("{} [iptables] rule removed for {}", ts(), naa_host);
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("iptables exited {}: {}", output.status, stderr.trim()))
    }
}
