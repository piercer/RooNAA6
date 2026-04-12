use crate::frame::build_pos_section;
use crate::metadata::PlayState;

#[test]
fn ends_with_null_terminator() {
    let section = build_pos_section(225, 11.0, PlayState::Playing, 2, 19);
    assert_eq!(*section.last().unwrap(), 0x00);
}

#[test]
fn contains_all_sixteen_fields_in_order() {
    let section = build_pos_section(225, 11.0, PlayState::Playing, 2, 19);
    let text = std::str::from_utf8(&section[..section.len() - 1]).unwrap();

    assert!(text.starts_with("[position]\n"));

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
fn playing_state_renders_as_playing() {
    let section = build_pos_section(225, 15.0, PlayState::Playing, 1, 1);
    let text = std::str::from_utf8(&section[..section.len() - 1]).unwrap();
    assert!(text.contains("state=PLAYING\n"));
    assert!(text.contains("position=15."), "text was: {text}");
}

#[test]
fn paused_state_renders_as_paused() {
    let section = build_pos_section(225, 42.5, PlayState::Paused, 1, 1);
    let text = std::str::from_utf8(&section[..section.len() - 1]).unwrap();
    assert!(text.contains("state=PAUSED\n"));
    assert!(text.contains("position=42.5"), "text was: {text}");
}

#[test]
fn derives_total_remain_begin_fields() {
    // length 3:45 = 225, position 11.0 → begin 0:11, total 3:45, remain 3:34
    let section = build_pos_section(225, 11.0, PlayState::Paused, 1, 1);
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
    // position 200s in a 180s track → should clamp to 180
    let section = build_pos_section(180, 200.0, PlayState::Playing, 1, 1);
    let text = std::str::from_utf8(&section[..section.len() - 1]).unwrap();

    assert!(text.contains("position=180."), "text was: {text}");
    assert!(text.contains("remain_min=0\n"));
    assert!(text.contains("remain_sec=0\n"));
}

#[test]
fn emits_track_and_tracks_total() {
    let section = build_pos_section(225, 0.0, PlayState::Playing, 2, 19);
    let text = std::str::from_utf8(&section[..section.len() - 1]).unwrap();

    assert!(text.contains("track=2\n"));
    assert!(text.contains("tracks_total=19\n"));
}

#[test]
fn emits_constant_fields() {
    let section = build_pos_section(225, 0.0, PlayState::Playing, 1, 1);
    let text = std::str::from_utf8(&section[..section.len() - 1]).unwrap();

    assert!(text.contains("apod=0\n"));
    assert!(text.contains("clips=0\n"));
    assert!(text.contains("correction=0\n"));
    assert!(text.contains("input_fill=-1.00000000000000000\n"));
    assert!(text.contains("output_fill=0.00000000000000000\n"));
}
