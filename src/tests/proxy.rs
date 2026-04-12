use std::sync::Arc;

use crate::frame::{FrameHeader, StreamParams, FRAME_HEADER_SIZE, TYPE_META, TYPE_PIC, TYPE_POS};
use crate::metadata::{Metadata, PlayState, SharedMetadata};
use crate::proxy::{Action, FrameOp, FrameProcessor};

const PCM: StreamParams = StreamParams { bits: 32, rate: 44100, is_dsd: false, bytes_per_sample: 4 };
const DSD: StreamParams = StreamParams { bits: 1, rate: 2822400, is_dsd: true, bytes_per_sample: 1 };

fn header(type_mask: u32, pcm_len: u32, pos_len: u32, meta_len: u32, pic_len: u32) -> FrameHeader {
    FrameHeader { raw: [0u8; FRAME_HEADER_SIZE], type_mask, pcm_len, pos_len, meta_len, pic_len }
}

fn meta(title: &str, artist: &str, album: &str, cover: Option<&[u8]>) -> Metadata {
    Metadata {
        title: title.to_string(),
        artist: artist.to_string(),
        album: album.to_string(),
        cover_art: cover.map(|c| Arc::new(c.to_vec())),
        length_seconds: None,
        seek_position: None,
        play_state: None,
        track: 0,
        tracks_total: 0,
    }
}

fn new_processor() -> FrameProcessor {
    FrameProcessor::new(SharedMetadata::new())
}

#[test]
fn new_defaults() {
    let proc = new_processor();
    assert!(proc.ops.is_empty());
    assert_eq!(proc.frame_count, 0);
    assert!(!proc.injected);
    assert!(proc.last_title.is_none());
    assert!(proc.last_pos_key.is_none());
    assert!(proc.pending_meta_pic.is_none());
    assert!(!proc.strip_logged);
    assert_eq!(proc.params.bits, 32);
    assert!(!proc.params.is_dsd);
}

#[test]
fn reset_clears_state() {
    let mut proc = new_processor();
    proc.injected = true;
    proc.last_title = Some("Old Song".into());
    proc.last_pos_key = Some((225, 10, PlayState::Playing));
    proc.frame_count = 500;
    proc.strip_logged = true;
    proc.pending_meta_pic = Some(vec![1, 2, 3]);
    proc.ops.push_back(FrameOp::Pass(100));
    proc.header_buf.extend_from_slice(&[0u8; 16]);

    proc.reset_for_start(DSD);

    assert!(proc.ops.is_empty());
    assert!(!proc.injected);
    assert!(proc.last_title.is_none());
    assert!(proc.last_pos_key.is_none());
    assert_eq!(proc.frame_count, 0);
    assert!(!proc.strip_logged);
    assert!(proc.header_buf.is_empty());
    assert!(proc.pending_meta_pic.is_none());
    assert_eq!(proc.params, DSD);
}

#[test]
fn inject_with_cover() {
    let mut proc = new_processor();
    proc.params = PCM;

    let fake_jpeg = vec![0xFF, 0xD8, 0x00, 0x01];
    let m = meta("Title", "Artist", "Album", Some(&fake_jpeg));
    let mut h = header(0x01, 100, 50, 0, 0);

    let jpeg_len = proc.inject(&mut h, &m, true);

    assert_eq!(jpeg_len, 4);
    assert_ne!(h.type_mask & TYPE_META, 0);
    assert_ne!(h.type_mask & TYPE_PIC, 0);
    assert_ne!(h.meta_len, 0);
    assert_eq!(h.pic_len, 4);
    assert_eq!(proc.last_title.as_deref(), Some("Title"));

    let payload = proc.pending_meta_pic.unwrap();
    assert!(payload.ends_with(&fake_jpeg));
}

#[test]
fn inject_without_cover() {
    let mut proc = new_processor();
    proc.params = PCM;

    let m = meta("Title", "Artist", "Album", Some(&[0xFF, 0xD8]));
    let mut h = header(0x01, 100, 50, 0, 0);

    let jpeg_len = proc.inject(&mut h, &m, false);

    assert_eq!(jpeg_len, 0);
    assert_ne!(h.type_mask & TYPE_META, 0);
    assert_eq!(h.type_mask & TYPE_PIC, 0);
    assert_eq!(h.pic_len, 0);

    let payload = proc.pending_meta_pic.unwrap();
    assert!(!payload.windows(2).any(|w| w == [0xFF, 0xD8]));
}

#[test]
fn inject_no_cover_art_available() {
    let mut proc = new_processor();
    proc.params = PCM;

    let m = meta("Title", "Artist", "Album", None);
    let mut h = header(0x01, 100, 50, 0, 0);

    let jpeg_len = proc.inject(&mut h, &m, true);

    assert_eq!(jpeg_len, 0);
    assert_ne!(h.type_mask & TYPE_META, 0);
    assert_eq!(h.type_mask & TYPE_PIC, 0);
    assert_eq!(h.pic_len, 0);
}

#[test]
fn inject_meta_section_content() {
    let mut proc = new_processor();
    proc.params = PCM;

    let m = meta("My Song", "My Artist", "My Album", None);
    let mut h = header(0x09, 100, 50, 200, 0);

    proc.inject(&mut h, &m, false);

    let payload = proc.pending_meta_pic.unwrap();
    let text = std::str::from_utf8(&payload[..payload.len() - 1]).unwrap();
    assert!(text.starts_with("[metadata]\n"));
    assert!(text.contains("song=My Song\n"));
    assert!(text.contains("artist=My Artist\n"));
    assert!(text.contains("album=My Album\n"));
    assert!(text.contains("samplerate=44100\n"));
    assert!(text.contains("sdm=0\n"));
}

#[test]
fn inject_dsd_uses_base_rate() {
    let mut proc = new_processor();
    proc.params = DSD;

    let m = meta("DSD Track", "Artist", "Album", None);
    let mut h = header(0x09, 100, 50, 200, 0);

    proc.inject(&mut h, &m, false);

    let payload = proc.pending_meta_pic.unwrap();
    let text = std::str::from_utf8(&payload[..payload.len() - 1]).unwrap();
    assert!(text.contains("samplerate=2822400\n"));
    assert!(text.contains("sdm=1\n"));
    assert!(text.contains("bits=1\n"));
}

#[test]
fn strip_clears_meta_and_pic() {
    let mut proc = new_processor();
    proc.pending_meta_pic = Some(vec![1, 2, 3]);

    let mut h = header(0x0D, 100, 50, 300, 5000);
    proc.strip(&mut h);

    assert_eq!(h.type_mask & TYPE_META, 0);
    assert_eq!(h.type_mask & TYPE_PIC, 0);
    assert_eq!(h.meta_len, 0);
    assert_eq!(h.pic_len, 0);
    assert!(proc.pending_meta_pic.is_none());
}

#[test]
fn decide_strip_when_hqp_meta_arrives_before_roon_title() {
    // Bug: HQPlayer's first META frame arrives before Roon WebSocket delivers
    // now_playing. Previously this fell through to PASSTHROUGH and HQP's
    // original title ("Roon") reached the T8.
    let proc = new_processor();
    assert!(!proc.injected);

    let action = proc.decide_action(/* has_meta */ true, /* title */ "");

    assert_eq!(action, Action::Strip);
}

#[test]
fn decide_inject_first_meta_frame_with_title() {
    let proc = new_processor();
    assert_eq!(proc.decide_action(true, "Song"), Action::Inject);
}

#[test]
fn decide_passthrough_audio_frame_no_title() {
    let proc = new_processor();
    assert_eq!(proc.decide_action(false, ""), Action::Passthrough);
}

#[test]
fn decide_inject_on_audio_frame_when_title_arrives_late() {
    // Title arrived after HQPlayer's META was already stripped.
    // We must inject on the next audio frame — there won't be another META.
    let proc = new_processor();
    assert_eq!(proc.decide_action(false, "Song"), Action::Inject);
}

#[test]
fn decide_gapless_on_track_change() {
    let mut proc = new_processor();
    proc.injected = true;
    proc.last_title = Some("Old".into());
    assert_eq!(proc.decide_action(false, "New"), Action::Gapless);
    assert_eq!(proc.decide_action(true, "New"), Action::Gapless);
}

#[test]
fn decide_strip_refresh_after_inject() {
    let mut proc = new_processor();
    proc.injected = true;
    proc.last_title = Some("Song".into());
    assert_eq!(proc.decide_action(true, "Song"), Action::Strip);
}

#[test]
fn decide_refresh_every_300_frames() {
    let mut proc = new_processor();
    proc.injected = true;
    proc.last_title = Some("Song".into());
    proc.frame_count = 300;
    assert_eq!(proc.decide_action(false, "Song"), Action::Refresh);
}

#[test]
fn decide_passthrough_between_refreshes() {
    let mut proc = new_processor();
    proc.injected = true;
    proc.last_title = Some("Song".into());
    proc.frame_count = 150;
    assert_eq!(proc.decide_action(false, "Song"), Action::Passthrough);
}

#[test]
fn strip_preserves_other_type_bits() {
    let mut proc = new_processor();

    // 0x1D = PCM(0x01) | POS(0x04) | META(0x08) | PIC(0x10)
    let mut h = header(0x1D, 100, 50, 300, 5000);
    proc.strip(&mut h);

    // strip() only clears META|PIC; POS bit is left to build_frame_ops.
    // After strip: PCM(0x01) | POS(0x04) = 0x05
    assert_eq!(h.type_mask, 0x05);
}

// --- build_frame_ops: event-driven POS injection ---

fn set_position(shared: &SharedMetadata, length: u32, seek: f64, state: PlayState) {
    let mut m = shared.get();
    m.length_seconds = Some(length);
    m.seek_position = Some(seek);
    m.play_state = Some(state);
    m.tracks_total = 5;
    shared.set(m);
}

fn body_header(pcm_len: u32, pos_len: u32) -> FrameHeader {
    FrameHeader {
        raw: [0u8; FRAME_HEADER_SIZE],
        type_mask: 0x01 | TYPE_POS, // PCM | POS
        pcm_len,
        pos_len,
        meta_len: 0,
        pic_len: 0,
    }
}

#[test]
fn build_frame_ops_emits_pos_on_first_sight() {
    let shared = SharedMetadata::new();
    set_position(&shared, 225, 10.0, PlayState::Playing);
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;
    proc.frame_count = 1;

    let mut header = body_header(100, 50);
    proc.build_frame_ops(&mut header);

    let ops: Vec<_> = proc.ops.iter().collect();
    assert!(
        matches!(ops[0], FrameOp::Pass(n) if *n == 100 * 4),
        "first op should pass PCM bytes, got {ops:?}"
    );
    assert!(
        matches!(ops[1], FrameOp::Skip(50)),
        "second op should skip orig pos_len, got {ops:?}"
    );
    assert!(
        matches!(ops[2], FrameOp::Emit(_)),
        "third op should emit new pos body, got {ops:?}"
    );
    assert!(header.pos_len > 0);
    assert_ne!(header.type_mask & TYPE_POS, 0);
    assert_eq!(proc.last_pos_key, Some((225, 10, PlayState::Playing)));
}

#[test]
fn build_frame_ops_strips_hqp_pos_when_no_shared_position() {
    // HQPlayer sends POS bytes but Roon hasn't populated position yet.
    // We must strip HQP's POS (which contains length=0 / garbage) rather
    // than passing it through.
    let shared = SharedMetadata::new();
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;
    proc.frame_count = 1;

    let mut header = body_header(100, 50);
    proc.build_frame_ops(&mut header);

    let ops: Vec<_> = proc.ops.iter().collect();
    assert!(
        matches!(ops[0], FrameOp::Pass(n) if *n == 100 * 4),
        "first op should pass PCM bytes only, got {ops:?}"
    );
    assert!(
        matches!(ops[1], FrameOp::Skip(50)),
        "second op should skip orig pos bytes, got {ops:?}"
    );
    assert!(proc.last_pos_key.is_none());
    assert_eq!(header.pos_len, 0);
    assert_eq!(header.type_mask & TYPE_POS, 0);
}

#[test]
fn build_frame_ops_skips_hqp_pos_when_key_unchanged() {
    // Once last_pos_key matches shared state, we stop emitting new POS
    // sections but still strip HQP's stale bytes every frame.
    let shared = SharedMetadata::new();
    set_position(&shared, 225, 10.0, PlayState::Playing);
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;
    proc.frame_count = 7;
    proc.last_pos_key = Some((225, 10, PlayState::Playing));

    let mut header = body_header(100, 50);
    proc.build_frame_ops(&mut header);

    let ops: Vec<_> = proc.ops.iter().collect();
    assert!(matches!(ops[0], FrameOp::Pass(n) if *n == 100 * 4));
    assert!(matches!(ops[1], FrameOp::Skip(50)));
    assert_eq!(
        ops.iter().filter(|op| matches!(op, FrameOp::Emit(_))).count(),
        0,
        "should not emit a new POS section when key is unchanged, got {ops:?}"
    );
    assert_eq!(header.pos_len, 0);
    assert_eq!(header.type_mask & TYPE_POS, 0);
}

#[test]
fn build_frame_ops_emits_pos_on_state_change() {
    let shared = SharedMetadata::new();
    set_position(&shared, 225, 10.0, PlayState::Paused);
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;
    proc.frame_count = 7;
    proc.last_pos_key = Some((225, 10, PlayState::Playing));

    let mut header = body_header(100, 50);
    proc.build_frame_ops(&mut header);

    let ops: Vec<_> = proc.ops.iter().collect();
    assert!(matches!(ops[0], FrameOp::Pass(n) if *n == 100 * 4));
    assert!(matches!(ops[1], FrameOp::Skip(50)));
    assert!(matches!(ops[2], FrameOp::Emit(_)));
    assert_eq!(proc.last_pos_key, Some((225, 10, PlayState::Paused)));
}

#[test]
fn build_frame_ops_emits_pos_on_seek_change() {
    let shared = SharedMetadata::new();
    set_position(&shared, 225, 42.0, PlayState::Playing);
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;
    proc.frame_count = 50;
    proc.last_pos_key = Some((225, 10, PlayState::Playing));

    let mut header = body_header(100, 50);
    proc.build_frame_ops(&mut header);

    let ops: Vec<_> = proc.ops.iter().collect();
    assert!(matches!(ops[0], FrameOp::Pass(_)));
    assert!(matches!(ops[1], FrameOp::Skip(50)));
    assert!(matches!(ops[2], FrameOp::Emit(_)));
    assert_eq!(proc.last_pos_key, Some((225, 42, PlayState::Playing)));
}

#[test]
fn build_frame_ops_pos_with_no_orig_pos_section() {
    let shared = SharedMetadata::new();
    set_position(&shared, 225, 10.0, PlayState::Playing);
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;
    proc.frame_count = 1;

    let mut header = body_header(100, 0);
    proc.build_frame_ops(&mut header);

    let ops: Vec<_> = proc.ops.iter().collect();
    assert!(matches!(ops[0], FrameOp::Pass(n) if *n == 100 * 4));
    assert!(matches!(ops[1], FrameOp::Emit(_)));
    assert!(header.pos_len > 0);
}
