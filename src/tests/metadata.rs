use crate::metadata::{Metadata, PlayState, SharedMetadata};

#[test]
fn metadata_default_is_empty() {
    let m = Metadata::default();
    assert!(m.length_seconds.is_none());
    assert!(m.seek_position.is_none());
    assert!(m.play_state.is_none());
    assert_eq!(m.title, "");
}

#[test]
fn shared_metadata_round_trip() {
    let shared = SharedMetadata::new();
    let mut m = (*shared.get()).clone();
    m.title = "Song".into();
    m.length_seconds = Some(100);
    m.seek_position = Some(0.0);
    m.play_state = Some(PlayState::Paused);
    shared.set(m);

    let out = shared.get();
    assert_eq!(out.title, "Song");
    assert_eq!(out.length_seconds, Some(100));
    assert_eq!(out.play_state, Some(PlayState::Paused));
}

#[test]
fn shared_metadata_partial_update_preserves_other_fields() {
    let shared = SharedMetadata::new();
    shared.set(Metadata {
        title: "Song".into(),
        length_seconds: Some(225),
        seek_position: Some(10.0),
        play_state: Some(PlayState::Playing),
        ..Metadata::default()
    });

    let mut m = (*shared.get()).clone();
    m.seek_position = Some(11.0);
    shared.set(m);

    let out = shared.get();
    assert_eq!(out.seek_position, Some(11.0));
    assert_eq!(out.title, "Song");
    assert_eq!(out.length_seconds, Some(225));
}
