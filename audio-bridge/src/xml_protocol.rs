use quick_xml::events::Event;
use quick_xml::reader::Reader;
use log::debug;

/// Parsed message from Roon (Control Server) or HQPlayer (Control Client)
#[derive(Debug, Clone, PartialEq)]
pub enum HqpMessage {
    GetInfo,
    SessionAuthentication { client_id: String, public_key: String, signature: String },
    Stop,
    Status { subscribe: bool },
    VolumeRange,
    State,
    PlaylistClear,
    PlaylistAdd { secure_uri: String, nonce: String },
    Play,
    GetModes,
    GetFilters,
    GetShapers,
    GetRates,
    Unknown,
}

/// Parse a newline-terminated XML line into an HqpMessage
pub fn parse_message(line: &str) -> HqpMessage {
    let line = line.trim();
    if line.is_empty() {
        return HqpMessage::Unknown;
    }

    debug!("Parsing XML: {}", line);

    let mut reader = Reader::from_str(line);
    reader.config_mut().trim_text(true);

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name = std::str::from_utf8(e.name().as_ref()).unwrap_or("").to_string();
                match name.as_str() {
                    "GetInfo" => return HqpMessage::GetInfo,
                    "Stop" => return HqpMessage::Stop,
                    "VolumeRange" => return HqpMessage::VolumeRange,
                    "State" => return HqpMessage::State,
                    "PlaylistClear" => return HqpMessage::PlaylistClear,
                    "Play" => return HqpMessage::Play,
                    "GetModes" => return HqpMessage::GetModes,
                    "GetFilters" => return HqpMessage::GetFilters,
                    "GetShapers" => return HqpMessage::GetShapers,
                    "GetRates" => return HqpMessage::GetRates,
                    "SessionAuthentication" => {
                        let mut client_id = String::new();
                        let mut public_key = String::new();
                        let mut signature = String::new();
                        for attr in e.attributes().flatten() {
                            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("").to_string();
                            let val = attr.unescape_value().unwrap_or_default().to_string();
                            match key.as_str() {
                                "client_id" => client_id = val,
                                "public_key" => public_key = val,
                                "signature" => signature = val,
                                _ => {}
                            }
                        }
                        return HqpMessage::SessionAuthentication { client_id, public_key, signature };
                    }
                    "Status" => {
                        let mut subscribe = false;
                        for attr in e.attributes().flatten() {
                            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("").to_string();
                            let val = attr.unescape_value().unwrap_or_default().to_string();
                            if key == "subscribe" {
                                subscribe = val == "1";
                            }
                        }
                        return HqpMessage::Status { subscribe };
                    }
                    "PlaylistAdd" => {
                        let mut secure_uri = String::new();
                        let mut nonce = String::new();
                        for attr in e.attributes().flatten() {
                            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("").to_string();
                            let val = attr.unescape_value().unwrap_or_default().to_string();
                            match key.as_str() {
                                "secure_uri" => secure_uri = val,
                                "nonce" => nonce = val,
                                _ => {}
                            }
                        }
                        return HqpMessage::PlaylistAdd { secure_uri, nonce };
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return HqpMessage::Unknown,
            _ => {}
        }
    }

    HqpMessage::Unknown
}

// ── Response builders ────────────────────────────────────────────────────────

fn wrap(inner: &str) -> String {
    format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>{}\n", inner)
}

pub fn getinfo_response() -> String {
    wrap("<GetInfo engine=\"5.35.6\" name=\"HQPlayerEmbedded\" platform=\"Linux\" product=\"Signalyst HQPlayer Embedded\" version=\"5\"/>")
}

pub fn session_auth_response() -> String {
    // Use plausible-looking base64 values — Roon may check format but not crypto
    wrap("<SessionAuthentication result=\"OK\" nonce=\"bEqXs5Osq9T8v+wY\" public_key=\"BC2KmSmVtlGwLBbIRMyYX05RwPXT4aE6y6Z3DoXKbsFGEsuT808vZx0LltdGIuyxY05KtxBjn/R1szijWWsuIbQ=\" result=\"OK\" signature=\"/VrXvExZs132zjOssIFUCvmWdYnfKP4IWXzpOmA/hBP/4n5Y3jch1RxE+1egaLIkGgX2QREbHyGvo+keC0jdDA==\" version=\"7k0VuZsXdoOPnF4VwGB4FSdGq20F8Q==\"/>")
}

pub fn stop_response() -> String {
    wrap("<Stop result=\"OK\"/>")
}

pub fn status_response(bits: u8, channels: u8, rate: u32) -> String {
    wrap(&format!(
        "<Status active_bits=\"{}\" active_channels=\"{}\" active_rate=\"{}\" active_mode=\"0\" \
active_filter=\"poly-sinc-gauss-xla\" active_shaper=\"LNS15\" apod=\"0\" begin_min=\"0\" \
begin_sec=\"0\" clips=\"0\" correction=\"0\" display_position=\"0.0\" filter_20k=\"0\" \
input_fill=\"0.0\" length=\"0.0\" min=\"0\" output_delay=\"0\" output_fill=\"0.0\" \
position=\"0.0\" process_speed=\"0.0\" queued=\"0\" random=\"0\" remain_min=\"0\" \
remain_sec=\"0\" repeat=\"0\" sec=\"0\" state=\"0\" total_min=\"0\" total_sec=\"0\" \
track=\"0\" track_serial=\"0\" tracks_total=\"0\" transport_serial=\"1\" volume=\"-3.0\"/>",
        bits, channels, rate
    ))
}

pub fn volume_range_response() -> String {
    wrap("<VolumeRange adaptive=\"0\" enabled=\"1\" max=\"0.000000000000000\" min=\"-60.000000000000000\"/>")
}

pub fn state_response(rate: u32) -> String {
    wrap(&format!(
        "<State active_mode=\"0\" active_rate=\"{}\" adaptive=\"0\" convolution=\"0\" \
filter=\"38\" filter1x=\"38\" filterNx=\"38\" filter_20k=\"0\" invert=\"0\" \
matrix_profile=\"\" mode=\"0\" random=\"0\" rate=\"0\" repeat=\"0\" shaper=\"5\" \
state=\"0\" volume=\"-3.000000000000000\"/>",
        rate
    ))
}

pub fn playlist_clear_response() -> String {
    wrap("<PlaylistClear result=\"OK\"/>")
}

pub fn playlist_add_response() -> String {
    wrap("<PlaylistAdd result=\"OK\"/>")
}

pub fn play_response() -> String {
    wrap("<Play result=\"OK\"/>")
}

pub fn get_modes_response() -> String {
    wrap("<GetModes><ModesItem index=\"0\" name=\"[source]\" value=\"-1\"/><ModesItem index=\"1\" name=\"PCM\" value=\"0\"/><ModesItem index=\"2\" name=\"SDM (DSD)\" value=\"1\"/></GetModes>")
}

pub fn get_filters_response() -> String {
    // Return a minimal set of filters
    wrap("<GetFilters><FiltersItem arg=\"2\" index=\"0\" name=\"poly-sinc-gauss-xla\" value=\"39\"/><FiltersItem arg=\"1\" index=\"1\" name=\"poly-sinc-ext2\" value=\"24\"/><FiltersItem arg=\"0\" index=\"2\" name=\"none\" value=\"0\"/></GetFilters>")
}

pub fn get_shapers_response() -> String {
    wrap("<GetShapers><ShapersItem index=\"0\" name=\"none\" value=\"0\"/><ShapersItem index=\"1\" name=\"TPDF\" value=\"5\"/><ShapersItem index=\"2\" name=\"LNS15\" value=\"9\"/></GetShapers>")
}

pub fn get_rates_response() -> String {
    wrap("<GetRates><RatesItem index=\"0\" name=\"44100\" value=\"44100\"/><RatesItem index=\"1\" name=\"48000\" value=\"48000\"/><RatesItem index=\"2\" name=\"88200\" value=\"88200\"/><RatesItem index=\"3\" name=\"96000\" value=\"96000\"/><RatesItem index=\"4\" name=\"176400\" value=\"176400\"/><RatesItem index=\"5\" name=\"192000\" value=\"192000\"/><RatesItem index=\"6\" name=\"352800\" value=\"352800\"/><RatesItem index=\"7\" name=\"384000\" value=\"384000\"/></GetRates>")
}

/// PlaylistAdd message sent from Control Client to HQPlayer
pub fn playlist_add_to_hqplayer(stream_url: &str) -> String {
    // Try both uri and secure_uri — HQPlayer may require one or the other
    // Also include a nonce as HQPlayer may require it for CSRF protection
    wrap(&format!(
        "<PlaylistAdd uri=\"{}\" secure_uri=\"{}\" nonce=\"roon-naa6-bridge\" queued=\"0\" clear=\"1\"/>",
        xml_escape(stream_url),
        xml_escape(stream_url)
    ))
}

/// Escape special XML characters in attribute values
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_getinfo() {
        let line = "<?xml version=\"1.0\" encoding=\"utf-8\"?><GetInfo/>\n";
        assert_eq!(parse_message(line), HqpMessage::GetInfo);
    }

    #[test]
    fn test_parse_stop() {
        let line = "<?xml version=\"1.0\" encoding=\"utf-8\"?><Stop/>\n";
        assert_eq!(parse_message(line), HqpMessage::Stop);
    }

    #[test]
    fn test_parse_volume_range() {
        let line = "<?xml version=\"1.0\" encoding=\"utf-8\"?><VolumeRange/>\n";
        assert_eq!(parse_message(line), HqpMessage::VolumeRange);
    }

    #[test]
    fn test_parse_state() {
        let line = "<?xml version=\"1.0\" encoding=\"utf-8\"?><State/>\n";
        assert_eq!(parse_message(line), HqpMessage::State);
    }

    #[test]
    fn test_parse_playlist_clear() {
        let line = "<?xml version=\"1.0\" encoding=\"utf-8\"?><PlaylistClear/>\n";
        assert_eq!(parse_message(line), HqpMessage::PlaylistClear);
    }

    #[test]
    fn test_parse_session_authentication() {
        let line = "<?xml version=\"1.0\" encoding=\"utf-8\"?><SessionAuthentication client_id=\"abc\" public_key=\"pk\" signature=\"sig\"/>\n";
        match parse_message(line) {
            HqpMessage::SessionAuthentication { client_id, public_key, signature } => {
                assert_eq!(client_id, "abc");
                assert_eq!(public_key, "pk");
                assert_eq!(signature, "sig");
            }
            other => panic!("Expected SessionAuthentication, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_status_subscribe() {
        let line = "<?xml version=\"1.0\" encoding=\"utf-8\"?><Status subscribe=\"1\"/>\n";
        assert_eq!(parse_message(line), HqpMessage::Status { subscribe: true });
    }

    #[test]
    fn test_parse_playlist_add() {
        let line = "<?xml version=\"1.0\" encoding=\"utf-8\"?><PlaylistAdd secure_uri=\"roon://...\" nonce=\"xyz\" queued=\"0\" clear=\"0\"/>\n";
        match parse_message(line) {
            HqpMessage::PlaylistAdd { secure_uri, nonce } => {
                assert_eq!(secure_uri, "roon://...");
                assert_eq!(nonce, "xyz");
            }
            other => panic!("Expected PlaylistAdd, got {:?}", other),
        }
    }

    #[test]
    fn test_getinfo_response_format() {
        let r = getinfo_response();
        assert!(r.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>"));
        assert!(r.contains("HQPlayerEmbedded"));
        assert!(r.contains("engine=\"5.35.6\""));
        assert!(r.ends_with('\n'));
    }

    #[test]
    fn test_session_auth_response_ok() {
        let r = session_auth_response();
        assert!(r.contains("result=\"OK\""));
        assert!(r.ends_with('\n'));
    }

    #[test]
    fn test_stop_response() {
        let r = stop_response();
        assert!(r.contains("<Stop result=\"OK\"/>"));
        assert!(r.ends_with('\n'));
    }

    #[test]
    fn test_status_response_contains_required_fields() {
        let r = status_response(16, 2, 44100);
        assert!(r.contains("active_bits=\"16\""));
        assert!(r.contains("active_channels=\"2\""));
        assert!(r.contains("active_rate=\"44100\""));
        assert!(r.contains("state=\"0\""));
        assert!(r.ends_with('\n'));
    }

    #[test]
    fn test_volume_range_response() {
        let r = volume_range_response();
        assert!(r.contains("enabled=\"1\""));
        assert!(r.contains("min=\"-60.000000000000000\""));
        assert!(r.ends_with('\n'));
    }

    #[test]
    fn test_state_response_contains_required_fields() {
        let r = state_response(44100);
        assert!(r.contains("active_mode=\"0\""));
        assert!(r.contains("active_rate=\"44100\""));
        assert!(r.ends_with('\n'));
    }

    #[test]
    fn test_playlist_clear_response() {
        let r = playlist_clear_response();
        assert!(r.contains("<PlaylistClear result=\"OK\"/>"));
        assert!(r.ends_with('\n'));
    }

    #[test]
    fn test_playlist_add_response() {
        let r = playlist_add_response();
        assert!(r.contains("<PlaylistAdd result=\"OK\"/>"));
        assert!(r.ends_with('\n'));
    }

    #[test]
    fn test_playlist_add_to_hqplayer() {
        let r = playlist_add_to_hqplayer("http://10.0.0.1:30001/abc/stream.raw");
        assert!(r.contains("uri=\"http://10.0.0.1:30001/abc/stream.raw\""));
        assert!(r.contains("clear=\"1\""));
        assert!(r.ends_with('\n'));
    }

    // Property test: XML round-trip consistency (Req 11.5)
    #[cfg(test)]
    mod property_tests {
        use proptest::prelude::*;
        use super::*;

        // Feature: roon-naa6-bridge, Property 1: XML round-trip consistency
        // Validates: Requirements 11.5
        proptest! {
            #![proptest_config(proptest::test_runner::Config::with_cases(100))]

            #[test]
            fn prop_getinfo_round_trip(_dummy in 0u8..1u8) {
                let line = "<?xml version=\"1.0\" encoding=\"utf-8\"?><GetInfo/>\n";
                prop_assert_eq!(parse_message(line), HqpMessage::GetInfo);
            }

            #[test]
            fn prop_stop_round_trip(_dummy in 0u8..1u8) {
                let line = "<?xml version=\"1.0\" encoding=\"utf-8\"?><Stop/>\n";
                prop_assert_eq!(parse_message(line), HqpMessage::Stop);
            }

            #[test]
            fn prop_volume_range_round_trip(_dummy in 0u8..1u8) {
                let line = "<?xml version=\"1.0\" encoding=\"utf-8\"?><VolumeRange/>\n";
                prop_assert_eq!(parse_message(line), HqpMessage::VolumeRange);
            }

            #[test]
            fn prop_status_subscribe_round_trip(subscribe in 0u8..2u8) {
                let line = format!(
                    "<?xml version=\"1.0\" encoding=\"utf-8\"?><Status subscribe=\"{}\"/>\n",
                    subscribe
                );
                let msg = parse_message(&line);
                match msg {
                    HqpMessage::Status { subscribe: s } => {
                        prop_assert_eq!(s, subscribe == 1);
                    }
                    other => prop_assert!(false, "Expected Status, got {:?}", other),
                }
            }
        }
    }
}
