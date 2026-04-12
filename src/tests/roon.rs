use std::time::Instant;

use serde_json::json;

use crate::metadata::PlayState;
use crate::roon::extract_playback_position;

fn zone(state: &str, seek: f64, length: u64) -> serde_json::Value {
    json!({
        "state": state,
        "now_playing": { "length": length, "seek_position": seek },
    })
}

#[test]
fn extracts_playing_zone() {
    let z = zone("playing", 42.0, 225);
    let (p, seek_present) = extract_playback_position(&z, Instant::now()).unwrap();
    assert_eq!(p.state, PlayState::Playing);
    assert_eq!(p.length_seconds, 225);
    assert_eq!(p.position_seconds, 42.0);
    assert!(seek_present);
}

#[test]
fn extracts_paused_zone() {
    let z = zone("paused", 11.0, 180);
    let (p, _) = extract_playback_position(&z, Instant::now()).unwrap();
    assert_eq!(p.state, PlayState::Paused);
}

#[test]
fn loading_state_maps_to_playing() {
    let z = zone("loading", 0.0, 200);
    let (p, _) = extract_playback_position(&z, Instant::now()).unwrap();
    assert_eq!(p.state, PlayState::Playing);
}

#[test]
fn stopped_state_returns_none() {
    let z = zone("stopped", 0.0, 0);
    assert!(extract_playback_position(&z, Instant::now()).is_none());
}

#[test]
fn missing_seek_position_bootstraps_and_flags() {
    // Roon does not always include seek_position in zone-level now_playing
    // updates — only at state transitions. Bootstrap to 0.0 and flag
    // seek_present=false so the handler can merge-preserve prior state.
    let z = json!({ "state": "playing", "now_playing": { "length": 200 } });
    let (p, seek_present) = extract_playback_position(&z, Instant::now()).unwrap();
    assert_eq!(p.position_seconds, 0.0);
    assert_eq!(p.length_seconds, 200);
    assert_eq!(p.state, PlayState::Playing);
    assert!(!seek_present);
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
    let (p, _) = extract_playback_position(&z, Instant::now()).unwrap();
    assert_eq!(p.tracks_total, 1);
}

use std::sync::Arc;

use crate::metadata::{Metadata, PlaybackPosition, SharedMetadata};
use crate::roon::apply_zones_seek;

fn seeded_shared(state: PlayState) -> SharedMetadata {
    let shared = SharedMetadata::new();
    shared.set(Metadata {
        title: "Song".into(),
        artist: "Artist".into(),
        album: "Album".into(),
        cover_art: Some(Arc::new(vec![0xFF, 0xD8])),
        position: Some(PlaybackPosition {
            length_seconds: 225,
            position_seconds: 10.0,
            captured_at: Instant::now(),
            state,
            track: 1,
            tracks_total: 3,
        }),
    });
    shared
}

#[test]
fn zones_seek_updates_position_and_captured_at() {
    let shared = seeded_shared(PlayState::Playing);
    let before = shared.get().position.unwrap().captured_at;

    let body = json!({
        "zones_seek_changed": [
            {
                "zone_id": "z1",
                "seek_position": 50.0,
                "queue_time_remaining": 175,
            }
        ]
    });
    apply_zones_seek(&shared, &body);

    let after = shared.get().position.unwrap();
    assert_eq!(after.position_seconds, 50.0);
    assert!(after.captured_at > before);
    assert_eq!(shared.get().title, "Song");
}

#[test]
fn zones_seek_preserves_paused_state() {
    let shared = seeded_shared(PlayState::Paused);
    let body = json!({
        "zones_seek_changed": [
            { "zone_id": "z1", "seek_position": 15.0 }
        ]
    });
    apply_zones_seek(&shared, &body);
    let p = shared.get().position.unwrap();
    assert_eq!(p.state, PlayState::Paused);
    assert_eq!(p.position_seconds, 15.0);
}

#[test]
fn zones_seek_noop_if_no_position_yet() {
    let shared = SharedMetadata::new();
    let body = json!({
        "zones_seek_changed": [
            { "zone_id": "z1", "seek_position": 15.0 }
        ]
    });
    apply_zones_seek(&shared, &body);
    assert!(shared.get().position.is_none());
}

#[test]
fn zones_seek_ignored_when_body_has_no_seek_array() {
    let shared = seeded_shared(PlayState::Playing);
    let body = json!({ "zones_changed": [] });
    apply_zones_seek(&shared, &body);
    assert_eq!(shared.get().position.unwrap().position_seconds, 10.0);
}
