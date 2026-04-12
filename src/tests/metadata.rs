use std::time::Instant;

use crate::metadata::{Metadata, PlayState, PlaybackPosition, SharedMetadata};

#[test]
fn metadata_default_has_no_position() {
    let m = Metadata::default();
    assert!(m.position.is_none());
    assert_eq!(m.title, "");
}

#[test]
fn playback_position_fields() {
    let now = Instant::now();
    let p = PlaybackPosition {
        length_seconds: 225,
        position_seconds: 11.5,
        captured_at: now,
        state: PlayState::Playing,
        track: 2,
        tracks_total: 19,
    };
    assert_eq!(p.length_seconds, 225);
    assert_eq!(p.state, PlayState::Playing);
    assert_ne!(p.state, PlayState::Paused);
}

#[test]
fn shared_metadata_carries_position() {
    let shared = SharedMetadata::new();
    let mut m = shared.get();
    m.title = "Song".into();
    m.position = Some(PlaybackPosition {
        length_seconds: 100,
        position_seconds: 0.0,
        captured_at: Instant::now(),
        state: PlayState::Paused,
        track: 1,
        tracks_total: 1,
    });
    shared.set(m);
    assert_eq!(shared.get().position.unwrap().state, PlayState::Paused);
}
