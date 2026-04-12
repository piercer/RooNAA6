use std::time::{Duration, Instant};

use crate::frame::build_pos_section;
use crate::metadata::{PlayState, PlaybackPosition};

fn position(state: PlayState, length: u32, pos: f64, captured_at: Instant) -> PlaybackPosition {
    PlaybackPosition {
        length_seconds: length,
        position_seconds: pos,
        captured_at,
        state,
        track: 2,
        tracks_total: 19,
    }
}

#[test]
fn ends_with_null_terminator() {
    let p = position(PlayState::Playing, 225, 11.0, Instant::now());
    let section = build_pos_section(&p, Instant::now());
    assert_eq!(*section.last().unwrap(), 0x00);
}

#[test]
fn contains_all_thirteen_fields_in_order() {
    let p = position(PlayState::Playing, 225, 11.0, Instant::now());
    let section = build_pos_section(&p, Instant::now());
    let text = std::str::from_utf8(&section[..section.len() - 1]).unwrap();

    assert!(text.starts_with("[position]\n"));

    // Verify field order matches HQPlayer's emitted layout
    let fields = [
        "apod=",
        "begin_min=",
        "begin_sec=",
        "clips=",
        "correction=",
        "input_fill=",
        "length=",
        "output_fill=",
        "position=",
        "remain_min=",
        "remain_sec=",
        "state=",
        "total_min=",
        "total_sec=",
        "track=",
        "tracks_total=",
    ];
    let mut cursor = 0;
    for field in fields {
        let idx = text[cursor..].find(field).expect(field);
        cursor += idx + field.len();
    }
}

#[test]
fn playing_state_advances_with_wall_clock() {
    let captured = Instant::now();
    let p = position(PlayState::Playing, 225, 10.0, captured);
    let later = captured + Duration::from_secs(5);

    let section = build_pos_section(&p, later);
    let text = std::str::from_utf8(&section[..section.len() - 1]).unwrap();

    assert!(text.contains("state=PLAYING\n"));
    // After 5 seconds of wall-clock, position should be ~15.0
    assert!(text.contains("position=15."), "text was: {text}");
}

#[test]
fn paused_state_freezes_position() {
    let captured = Instant::now();
    let p = position(PlayState::Paused, 225, 42.5, captured);
    let later = captured + Duration::from_secs(10);

    let section = build_pos_section(&p, later);
    let text = std::str::from_utf8(&section[..section.len() - 1]).unwrap();

    assert!(text.contains("state=PAUSED\n"));
    assert!(text.contains("position=42.5"), "text was: {text}");
}

#[test]
fn derives_total_remain_begin_fields() {
    let captured = Instant::now();
    // length 3:45 = 225, position 11.0 → begin 0:11, total 3:45, remain 3:34
    let p = position(PlayState::Paused, 225, 11.0, captured);

    let section = build_pos_section(&p, captured);
    let text = std::str::from_utf8(&section[..section.len() - 1]).unwrap();

    assert!(text.contains("total_min=3\n"));
    assert!(text.contains("total_sec=45\n"));
    assert!(text.contains("remain_min=3\n"));
    assert!(text.contains("remain_sec=34\n"));
    assert!(text.contains("begin_min=0\n"));
    assert!(text.contains("begin_sec=11\n"));
}

#[test]
fn position_clamped_to_length_if_stale() {
    let captured = Instant::now();
    // position 200s in a 180s track → should clamp to 180
    let p = position(PlayState::Playing, 180, 200.0, captured);

    let section = build_pos_section(&p, captured);
    let text = std::str::from_utf8(&section[..section.len() - 1]).unwrap();

    // position should not exceed length
    assert!(text.contains("position=180."), "text was: {text}");
    // remain should not underflow
    assert!(text.contains("remain_min=0\n"));
    assert!(text.contains("remain_sec=0\n"));
}

#[test]
fn emits_track_and_tracks_total() {
    let p = position(PlayState::Playing, 225, 0.0, Instant::now());
    let section = build_pos_section(&p, Instant::now());
    let text = std::str::from_utf8(&section[..section.len() - 1]).unwrap();

    assert!(text.contains("track=2\n"));
    assert!(text.contains("tracks_total=19\n"));
}

#[test]
fn emits_constant_fields() {
    let p = position(PlayState::Playing, 225, 0.0, Instant::now());
    let section = build_pos_section(&p, Instant::now());
    let text = std::str::from_utf8(&section[..section.len() - 1]).unwrap();

    assert!(text.contains("apod=0\n"));
    assert!(text.contains("clips=0\n"));
    assert!(text.contains("correction=0\n"));
    assert!(text.contains("input_fill=-1.00000000000000000\n"));
    assert!(text.contains("output_fill=0.00000000000000000\n"));
}
