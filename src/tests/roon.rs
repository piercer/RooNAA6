use std::time::Instant;

use serde_json::json;

use crate::metadata::PlayState;
use crate::roon::extract_playback_position;

fn zone(state: &str, seek: f64, length: u64) -> serde_json::Value {
    json!({
        "state": state,
        "seek_position": seek,
        "now_playing": { "length": length },
    })
}

#[test]
fn extracts_playing_zone() {
    let z = zone("playing", 42.0, 225);
    let p = extract_playback_position(&z, Instant::now()).unwrap();
    assert_eq!(p.state, PlayState::Playing);
    assert_eq!(p.length_seconds, 225);
    assert_eq!(p.position_seconds, 42.0);
}

#[test]
fn extracts_paused_zone() {
    let z = zone("paused", 11.0, 180);
    let p = extract_playback_position(&z, Instant::now()).unwrap();
    assert_eq!(p.state, PlayState::Paused);
}

#[test]
fn loading_state_maps_to_playing() {
    let z = zone("loading", 0.0, 200);
    let p = extract_playback_position(&z, Instant::now()).unwrap();
    assert_eq!(p.state, PlayState::Playing);
}

#[test]
fn stopped_state_returns_none() {
    let z = zone("stopped", 0.0, 0);
    assert!(extract_playback_position(&z, Instant::now()).is_none());
}

#[test]
fn missing_seek_position_returns_none() {
    let z = json!({ "state": "playing", "now_playing": { "length": 200 } });
    assert!(extract_playback_position(&z, Instant::now()).is_none());
}

#[test]
fn missing_length_returns_none() {
    let z = json!({ "state": "playing", "seek_position": 10.0, "now_playing": {} });
    assert!(extract_playback_position(&z, Instant::now()).is_none());
}

#[test]
fn missing_now_playing_returns_none() {
    let z = json!({ "state": "playing", "seek_position": 10.0 });
    assert!(extract_playback_position(&z, Instant::now()).is_none());
}

#[test]
fn tracks_total_defaults_when_missing() {
    let z = zone("playing", 0.0, 200);
    let p = extract_playback_position(&z, Instant::now()).unwrap();
    assert_eq!(p.tracks_total, 1);
}
