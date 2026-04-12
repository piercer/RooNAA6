use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use crate::metadata::{Metadata, PlayState, SharedMetadata};
use crate::roon::{apply_zone_update, apply_zones_seek};

fn dummy_agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(Duration::from_millis(1)))
            .build(),
    )
}

fn apply(shared: &SharedMetadata, zone: &serde_json::Value) {
    let mut key = String::new();
    apply_zone_update(shared, zone, &dummy_agent(), "127.0.0.1", 1, &mut key);
}

#[test]
fn zone_update_maps_playing_state() {
    let shared = SharedMetadata::new();
    let zone = json!({
        "display_name": "ignored",
        "state": "playing",
        "seek_position": 42.0,
        "now_playing": {
            "length": 225,
            "three_line": { "line1": "Song", "line2": "Artist", "line3": "Album" },
        },
    });
    apply(&shared, &zone);

    let m = shared.get();
    assert_eq!(m.title, "Song");
    assert_eq!(m.artist, "Artist");
    assert_eq!(m.album, "Album");
    assert_eq!(m.length_seconds, Some(225));
    assert_eq!(m.seek_position, Some(42.0));
    assert_eq!(m.play_state, Some(PlayState::Playing));
}

#[test]
fn zone_update_maps_paused_state() {
    let shared = SharedMetadata::new();
    let zone = json!({
        "state": "paused",
        "seek_position": 11.0,
        "now_playing": { "length": 180, "three_line": { "line1": "X" } },
    });
    apply(&shared, &zone);
    assert_eq!(shared.get().play_state, Some(PlayState::Paused));
}

#[test]
fn zone_update_loading_maps_to_playing() {
    let shared = SharedMetadata::new();
    let zone = json!({
        "state": "loading",
        "seek_position": 0.0,
        "now_playing": { "length": 200, "three_line": { "line1": "X" } },
    });
    apply(&shared, &zone);
    assert_eq!(shared.get().play_state, Some(PlayState::Playing));
}

#[test]
fn zone_update_stopped_clears_play_state() {
    let shared = SharedMetadata::new();
    shared.set(Metadata {
        play_state: Some(PlayState::Playing),
        ..Metadata::default()
    });
    let zone = json!({ "state": "stopped", "now_playing": {} });
    apply(&shared, &zone);
    assert!(shared.get().play_state.is_none());
}

#[test]
fn zone_update_partial_preserves_prior_seek() {
    // A zones_changed event that omits seek_position must NOT wipe the
    // last-known seek value from shared state.
    let shared = SharedMetadata::new();
    shared.set(Metadata {
        seek_position: Some(99.0),
        ..Metadata::default()
    });
    let zone = json!({
        "state": "playing",
        "now_playing": { "length": 200, "three_line": { "line1": "X" } },
    });
    apply(&shared, &zone);
    assert_eq!(shared.get().seek_position, Some(99.0));
}

#[test]
fn zone_update_partial_preserves_prior_title() {
    // An event carrying only state info must not blank out title/artist/album.
    let shared = SharedMetadata::new();
    shared.set(Metadata {
        title: "Old Song".into(),
        artist: "Old Artist".into(),
        album: "Old Album".into(),
        ..Metadata::default()
    });
    // Note: apply_zone_update unconditionally rewrites title/artist/album
    // when now_playing is present. This test guards the case where
    // now_playing is absent — a pure state-only update.
    let zone = json!({ "state": "paused" });
    apply(&shared, &zone);
    let m = shared.get();
    assert_eq!(m.title, "Old Song");
    assert_eq!(m.artist, "Old Artist");
    assert_eq!(m.album, "Old Album");
    assert_eq!(m.play_state, Some(PlayState::Paused));
}

#[test]
fn zone_update_seek_position_fallback_from_now_playing() {
    let shared = SharedMetadata::new();
    let zone = json!({
        "state": "playing",
        "now_playing": {
            "length": 200,
            "seek_position": 17.0,
            "three_line": { "line1": "X" },
        },
    });
    apply(&shared, &zone);
    assert_eq!(shared.get().seek_position, Some(17.0));
}

#[test]
fn zone_update_preserves_cover_when_image_key_unchanged() {
    let shared = SharedMetadata::new();
    shared.set(Metadata {
        cover_art: Some(Arc::new(vec![0xFF, 0xD8, 0x42])),
        ..Metadata::default()
    });
    let mut key = "img-1".to_string();
    let zone = json!({
        "state": "playing",
        "now_playing": {
            "length": 200,
            "image_key": "img-1",
            "three_line": { "line1": "X" },
        },
    });
    apply_zone_update(&shared, &zone, &dummy_agent(), "127.0.0.1", 1, &mut key);
    assert_eq!(
        shared.get().cover_art.as_deref().map(|v| v.as_slice()),
        Some([0xFF, 0xD8, 0x42].as_slice())
    );
}

#[test]
fn zones_seek_updates_seek_position() {
    let shared = SharedMetadata::new();
    shared.set(Metadata {
        title: "Song".into(),
        seek_position: Some(10.0),
        length_seconds: Some(225),
        play_state: Some(PlayState::Playing),
        ..Metadata::default()
    });
    let body = json!({
        "zones_seek_changed": [
            { "zone_id": "z1", "seek_position": 50.0, "queue_time_remaining": 175 }
        ]
    });
    apply_zones_seek(&shared, &body);

    let m = shared.get();
    assert_eq!(m.seek_position, Some(50.0));
    assert_eq!(m.title, "Song");
    assert_eq!(m.length_seconds, Some(225));
    assert_eq!(m.play_state, Some(PlayState::Playing));
}

#[test]
fn zones_seek_preserves_paused_state() {
    let shared = SharedMetadata::new();
    shared.set(Metadata {
        seek_position: Some(10.0),
        play_state: Some(PlayState::Paused),
        ..Metadata::default()
    });
    let body = json!({
        "zones_seek_changed": [{ "zone_id": "z1", "seek_position": 15.0 }]
    });
    apply_zones_seek(&shared, &body);
    let m = shared.get();
    assert_eq!(m.play_state, Some(PlayState::Paused));
    assert_eq!(m.seek_position, Some(15.0));
}

#[test]
fn zones_seek_noop_when_no_seek_array() {
    let shared = SharedMetadata::new();
    shared.set(Metadata {
        seek_position: Some(10.0),
        ..Metadata::default()
    });
    let body = json!({ "zones_changed": [] });
    apply_zones_seek(&shared, &body);
    assert_eq!(shared.get().seek_position, Some(10.0));
}

#[test]
fn zones_seek_noop_when_entry_has_no_seek() {
    let shared = SharedMetadata::new();
    shared.set(Metadata {
        seek_position: Some(10.0),
        ..Metadata::default()
    });
    let body = json!({ "zones_seek_changed": [{ "zone_id": "z1" }] });
    apply_zones_seek(&shared, &body);
    assert_eq!(shared.get().seek_position, Some(10.0));
}
