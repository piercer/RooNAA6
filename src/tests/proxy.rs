use std::sync::Arc;

use crate::frame::{FrameHeader, StreamParams, FRAME_HEADER_SIZE, TYPE_META, TYPE_PIC, TYPE_POS};
use crate::metadata::{Metadata, PlayState, SharedMetadata};
use crate::proxy::{FrameOp, FrameProcessor};

const PCM: StreamParams = StreamParams { bits: 32, rate: 44100, is_dsd: false, bytes_per_sample: 4 };
const DSD: StreamParams = StreamParams { bits: 1, rate: 2822400, is_dsd: true, bytes_per_sample: 1 };

fn header(type_mask: u32, pcm_len: u32, pos_len: u32, meta_len: u32, pic_len: u32) -> FrameHeader {
    FrameHeader { raw: [0u8; FRAME_HEADER_SIZE], type_mask, pcm_len, pos_len, meta_len, pic_len }
}

fn seed_track(shared: &SharedMetadata, title: &str, cover: Option<&[u8]>) {
    shared.set(Metadata {
        title: title.into(),
        artist: "Artist".into(),
        album: "Album".into(),
        cover_art: cover.map(|c| Arc::new(c.to_vec())),
        ..Metadata::default()
    });
}

fn seed_position(shared: &SharedMetadata, length: u32, seek: f64, state: PlayState) {
    let mut m = shared.get();
    m.length_seconds = Some(length);
    m.seek_position = Some(seek);
    m.play_state = Some(state);
    m.tracks_total = 5;
    shared.set(m);
}

#[test]
fn new_defaults() {
    let proc = FrameProcessor::new(SharedMetadata::new());
    assert!(proc.ops.is_empty());
    assert_eq!(proc.frame_count, 0);
    assert!(proc.last_meta_key.is_none());
    assert!(proc.last_pos_key.is_none());
    assert_eq!(proc.params.bits, 32);
    assert!(!proc.params.is_dsd);
}

#[test]
fn reset_for_start_clears_all_keys() {
    let mut proc = FrameProcessor::new(SharedMetadata::new());
    proc.last_meta_key = Some(("t".into(), "a".into(), "b".into()));
    proc.last_pos_key = Some((225, 10, PlayState::Playing));
    proc.frame_count = 500;
    proc.ops.push_back(FrameOp::Pass(100));
    proc.header_buf.extend_from_slice(&[0u8; 16]);

    proc.reset_for_start(DSD);

    assert!(proc.last_meta_key.is_none());
    assert!(proc.last_pos_key.is_none());
    assert_eq!(proc.frame_count, 0);
    assert_eq!(proc.params, DSD);
    assert!(proc.ops.is_empty());
    assert!(proc.header_buf.is_empty());
}

// --- META/PIC: strip-always + event-driven ---

#[test]
fn strips_hqp_meta_when_no_roon_title_yet() {
    // HQPlayer sent META/PIC (fallback "Roon") but Roon state is empty.
    // Proxy must unconditionally strip, emitting nothing.
    let shared = SharedMetadata::new();
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;

    let mut h = header(0x01 | TYPE_META | TYPE_PIC, 100, 0, 200, 5000);
    proc.build_frame_ops(&mut h);

    assert_eq!(h.type_mask & TYPE_META, 0);
    assert_eq!(h.type_mask & TYPE_PIC, 0);
    assert_eq!(h.meta_len, 0);
    assert_eq!(h.pic_len, 0);

    let ops: Vec<_> = proc.ops.iter().collect();
    assert!(matches!(ops[0], FrameOp::Pass(n) if *n == 100 * 4));
    assert!(matches!(ops[1], FrameOp::Skip(n) if *n == 200 + 5000));
    assert_eq!(
        ops.iter().filter(|op| matches!(op, FrameOp::Emit(_))).count(),
        0,
    );
    assert!(proc.last_meta_key.is_none());
}

#[test]
fn emits_meta_on_first_sight_when_title_known() {
    let shared = SharedMetadata::new();
    seed_track(&shared, "Song", Some(&[0xFF, 0xD8, 0xAA, 0xBB]));
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;

    // Audio-only frame (no HQP META bit).
    let mut h = header(0x01, 100, 0, 0, 0);
    proc.build_frame_ops(&mut h);

    assert_ne!(h.type_mask & TYPE_META, 0);
    assert_ne!(h.type_mask & TYPE_PIC, 0);
    assert_eq!(h.pic_len, 4);
    assert!(h.meta_len > 0);
    assert_eq!(
        proc.last_meta_key,
        Some(("Song".into(), "Artist".into(), "Album".into()))
    );

    let ops: Vec<_> = proc.ops.iter().collect();
    assert!(matches!(ops[0], FrameOp::Pass(n) if *n == 100 * 4));
    // Two emits: meta section then cover bytes.
    assert!(matches!(ops[1], FrameOp::Emit(_)));
    assert!(matches!(ops[2], FrameOp::Emit(_)));
}

#[test]
fn reemits_meta_every_frame_without_cover_when_key_unchanged() {
    // META text is re-sent on every frame once the title is known,
    // so the T8 never reverts to HQPlayer's "Roon" fallback. Cover
    // stays strictly change-driven — no re-send while the key holds.
    let shared = SharedMetadata::new();
    seed_track(&shared, "Song", Some(&vec![0xFFu8; 40000]));
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;
    proc.last_meta_key = Some(("Song".into(), "Artist".into(), "Album".into()));
    proc.frame_count = 50;

    // HQP sent META bytes — must be stripped, and replaced with ours.
    let mut h = header(0x01 | TYPE_META, 100, 0, 250, 0);
    proc.build_frame_ops(&mut h);

    assert_ne!(h.type_mask & TYPE_META, 0);
    assert_eq!(h.type_mask & TYPE_PIC, 0);
    assert_eq!(h.pic_len, 0);
    assert!(h.meta_len > 0);

    let ops: Vec<_> = proc.ops.iter().collect();
    assert!(matches!(ops[0], FrameOp::Pass(n) if *n == 100 * 4));
    assert!(matches!(ops[1], FrameOp::Skip(250)));
    let emits = ops.iter().filter(|op| matches!(op, FrameOp::Emit(_))).count();
    assert_eq!(emits, 1, "re-emit meta text only, no cover");
}

#[test]
fn emits_new_meta_with_cover_on_track_change() {
    let shared = SharedMetadata::new();
    seed_track(&shared, "New Song", Some(&[0xFF, 0xD8]));
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;
    proc.last_meta_key = Some(("Old Song".into(), "Artist".into(), "Album".into()));
    proc.frame_count = 50;

    let mut h = header(0x01, 100, 0, 0, 0);
    proc.build_frame_ops(&mut h);

    assert_ne!(h.type_mask & TYPE_META, 0);
    assert_ne!(h.type_mask & TYPE_PIC, 0);
    assert_eq!(h.pic_len, 2);
    assert_eq!(
        proc.last_meta_key,
        Some(("New Song".into(), "Artist".into(), "Album".into()))
    );
    let emits = proc.ops.iter().filter(|op| matches!(op, FrameOp::Emit(_))).count();
    assert_eq!(emits, 2, "content change should emit meta + cover");
}

fn find_meta_emit<'a>(proc: &'a FrameProcessor) -> &'a [u8] {
    // The META section Emit is the first one whose bytes start with the
    // `[metadata]` header — always present when emission occurred.
    for op in proc.ops.iter() {
        if let FrameOp::Emit(b) = op {
            if b.starts_with(b"[metadata]") {
                return b;
            }
        }
    }
    panic!("no meta section Emit in ops: {:?}", proc.ops);
}

#[test]
fn meta_payload_contains_track_fields() {
    let shared = SharedMetadata::new();
    shared.set(Metadata {
        title: "My Song".into(),
        artist: "My Artist".into(),
        album: "My Album".into(),
        ..Metadata::default()
    });
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;

    let mut h = header(0x01, 100, 0, 0, 0);
    proc.build_frame_ops(&mut h);

    let section = find_meta_emit(&proc);
    let text = std::str::from_utf8(&section[..section.len() - 1]).unwrap();
    assert!(text.contains("song=My Song\n"));
    assert!(text.contains("artist=My Artist\n"));
    assert!(text.contains("album=My Album\n"));
    assert!(text.contains("samplerate=44100\n"));
}

#[test]
fn meta_payload_uses_dsd_base_rate() {
    let shared = SharedMetadata::new();
    seed_track(&shared, "DSD Track", None);
    let mut proc = FrameProcessor::new(shared);
    proc.params = DSD;

    let mut h = header(0x01, 100, 0, 0, 0);
    proc.build_frame_ops(&mut h);

    let section = find_meta_emit(&proc);
    let text = std::str::from_utf8(&section[..section.len() - 1]).unwrap();
    assert!(text.contains("samplerate=2822400\n"));
    assert!(text.contains("sdm=1\n"));
}

#[test]
fn meta_header_len_excludes_jpeg() {
    // meta_len advertises only the [metadata] section size; pic_len covers
    // the JPEG. The two Emits are separate ops on the wire.
    let shared = SharedMetadata::new();
    let jpeg = vec![0xFFu8; 1234];
    seed_track(&shared, "Song", Some(&jpeg));
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;

    let mut h = header(0x01, 100, 0, 0, 0);
    proc.build_frame_ops(&mut h);

    assert_eq!(h.pic_len, 1234);
    assert!(h.meta_len > 0);
    assert!(h.meta_len < 500, "meta_len should not include jpeg bytes");
}

// --- POS: strip-always + event-driven ---

#[test]
fn emits_pos_on_first_sight() {
    let shared = SharedMetadata::new();
    seed_position(&shared, 225, 10.0, PlayState::Playing);
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;

    let mut h = header(0x01 | TYPE_POS, 100, 50, 0, 0);
    proc.build_frame_ops(&mut h);

    let ops: Vec<_> = proc.ops.iter().collect();
    assert!(matches!(ops[0], FrameOp::Pass(n) if *n == 100 * 4));
    assert!(matches!(ops[1], FrameOp::Skip(50)));
    assert!(matches!(ops[2], FrameOp::Emit(_)));
    assert!(h.pos_len > 0);
    assert_ne!(h.type_mask & TYPE_POS, 0);
    assert_eq!(proc.last_pos_key, Some((225, 10, PlayState::Playing)));
}

#[test]
fn strips_hqp_pos_when_no_shared_position() {
    let shared = SharedMetadata::new();
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;

    let mut h = header(0x01 | TYPE_POS, 100, 50, 0, 0);
    proc.build_frame_ops(&mut h);

    let ops: Vec<_> = proc.ops.iter().collect();
    assert!(matches!(ops[0], FrameOp::Pass(n) if *n == 100 * 4));
    assert!(matches!(ops[1], FrameOp::Skip(50)));
    assert!(proc.last_pos_key.is_none());
    assert_eq!(h.pos_len, 0);
    assert_eq!(h.type_mask & TYPE_POS, 0);
}

#[test]
fn skips_hqp_pos_when_key_unchanged() {
    let shared = SharedMetadata::new();
    seed_position(&shared, 225, 10.0, PlayState::Playing);
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;
    proc.last_pos_key = Some((225, 10, PlayState::Playing));

    let mut h = header(0x01 | TYPE_POS, 100, 50, 0, 0);
    proc.build_frame_ops(&mut h);

    let ops: Vec<_> = proc.ops.iter().collect();
    assert!(matches!(ops[0], FrameOp::Pass(n) if *n == 100 * 4));
    assert!(matches!(ops[1], FrameOp::Skip(50)));
    assert_eq!(
        ops.iter().filter(|op| matches!(op, FrameOp::Emit(_))).count(),
        0,
    );
    assert_eq!(h.pos_len, 0);
    assert_eq!(h.type_mask & TYPE_POS, 0);
}

#[test]
fn emits_pos_on_state_change() {
    let shared = SharedMetadata::new();
    seed_position(&shared, 225, 10.0, PlayState::Paused);
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;
    proc.last_pos_key = Some((225, 10, PlayState::Playing));

    let mut h = header(0x01 | TYPE_POS, 100, 50, 0, 0);
    proc.build_frame_ops(&mut h);

    assert_eq!(proc.last_pos_key, Some((225, 10, PlayState::Paused)));
    let ops: Vec<_> = proc.ops.iter().collect();
    assert!(ops.iter().any(|op| matches!(op, FrameOp::Emit(_))));
}

#[test]
fn emits_pos_on_seek_change() {
    let shared = SharedMetadata::new();
    seed_position(&shared, 225, 42.0, PlayState::Playing);
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;
    proc.last_pos_key = Some((225, 10, PlayState::Playing));

    let mut h = header(0x01 | TYPE_POS, 100, 50, 0, 0);
    proc.build_frame_ops(&mut h);

    assert_eq!(proc.last_pos_key, Some((225, 42, PlayState::Playing)));
    let ops: Vec<_> = proc.ops.iter().collect();
    assert!(ops.iter().any(|op| matches!(op, FrameOp::Emit(_))));
}

#[test]
fn no_orig_pos_section_still_emits_when_needed() {
    let shared = SharedMetadata::new();
    seed_position(&shared, 225, 10.0, PlayState::Playing);
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;

    let mut h = header(0x01, 100, 0, 0, 0);
    proc.build_frame_ops(&mut h);

    let ops: Vec<_> = proc.ops.iter().collect();
    assert!(matches!(ops[0], FrameOp::Pass(n) if *n == 100 * 4));
    // No Skip(orig_pos) because orig_pos_len == 0.
    assert!(matches!(ops[1], FrameOp::Emit(_)));
    assert!(h.pos_len > 0);
}

// --- Combined: both slots owned uniformly ---

#[test]
fn strips_all_owned_sections_uniformly() {
    // A frame carrying HQP's POS, META, and PIC with no Roon state.
    // All three must be stripped; type_mask should retain only PCM.
    let shared = SharedMetadata::new();
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;

    let mut h = header(
        0x01 | TYPE_POS | TYPE_META | TYPE_PIC,
        100,
        50,
        200,
        1000,
    );
    proc.build_frame_ops(&mut h);

    assert_eq!(h.type_mask, 0x01);
    assert_eq!(h.pos_len, 0);
    assert_eq!(h.meta_len, 0);
    assert_eq!(h.pic_len, 0);
}

#[test]
fn emits_both_slots_when_both_keys_change() {
    let shared = SharedMetadata::new();
    seed_track(&shared, "Song", Some(&[0xFF, 0xD8]));
    seed_position(&shared, 225, 10.0, PlayState::Playing);
    let mut proc = FrameProcessor::new(shared);
    proc.params = PCM;

    let mut h = header(0x01 | TYPE_POS | TYPE_META, 100, 50, 100, 0);
    proc.build_frame_ops(&mut h);

    // POS and META/PIC both emitted; header bits all set.
    assert_ne!(h.type_mask & TYPE_POS, 0);
    assert_ne!(h.type_mask & TYPE_META, 0);
    assert_ne!(h.type_mask & TYPE_PIC, 0);

    // Three emits: pos section + meta section + cover bytes.
    let emits = proc.ops.iter().filter(|op| matches!(op, FrameOp::Emit(_))).count();
    assert_eq!(emits, 3);
}
