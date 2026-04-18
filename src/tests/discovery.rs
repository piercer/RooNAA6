use std::net::SocketAddr;

use crate::discovery::parse_discover_response;

fn sample_response(name: &str, version: &str) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<networkaudio>\
<discover result=\"OK\" name=\"{name}\" version=\"{version}\" protocol=\"6\" trigger=\"0\">\
network audio\
</discover>\
</networkaudio>\n",
    )
    .into_bytes()
}

fn addr(ip: &str) -> SocketAddr {
    format!("{ip}:43210").parse().unwrap()
}

#[test]
fn parses_valid_response() {
    let data = sample_response("T+A DAC 8 DSD", "ta naa");
    let ep = parse_discover_response(&data, addr("192.168.30.109")).unwrap();
    assert_eq!(ep.name, "T+A DAC 8 DSD");
    assert_eq!(ep.version, "ta naa");
    assert_eq!(ep.protocol, "6");
    assert_eq!(ep.trigger, "0");
    assert_eq!(ep.addr.ip().to_string(), "192.168.30.109");
    assert_eq!(ep.addr.port(), 43210);
}

#[test]
fn returns_none_for_query_message() {
    let query = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<networkaudio><discover>network audio</discover></networkaudio>\n";
    assert!(parse_discover_response(query, addr("192.168.30.1")).is_none());
}

#[test]
fn returns_none_for_non_utf8() {
    let data = &[0xff, 0xfe, 0x00, 0x01];
    assert!(parse_discover_response(data, addr("192.168.30.1")).is_none());
}

#[test]
fn returns_none_when_missing_name() {
    let data = b"<?xml version=\"1.0\"?>\
<networkaudio><discover result=\"OK\" version=\"naa\" protocol=\"6\">\
network audio</discover></networkaudio>\n";
    assert!(parse_discover_response(data, addr("192.168.30.1")).is_none());
}

#[test]
fn defaults_protocol_to_6() {
    let data = b"<?xml version=\"1.0\"?>\
<networkaudio><discover result=\"OK\" name=\"Test\" version=\"naa\">\
network audio</discover></networkaudio>\n";
    let ep = parse_discover_response(data, addr("10.0.0.1")).unwrap();
    assert_eq!(ep.protocol, "6");
}

#[test]
fn addr_port_is_always_naa_port() {
    let data = sample_response("Test", "naa");
    let ep = parse_discover_response(&data, "10.0.0.5:9999".parse().unwrap()).unwrap();
    assert_eq!(ep.addr.port(), 43210);
}
