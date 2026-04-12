use std::sync::Arc;
use std::time::Instant;

use crate::frame::{FrameHeader, StreamParams, FRAME_HEADER_SIZE, TYPE_META, TYPE_PIC};
use crate::metadata::{Metadata, PlayState, PlaybackPosition, SharedMetadata};
use crate::proxy::{Action, FrameProcessor, PosAction};

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
        position: None,
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
    assert!(proc.last_pos_state.is_none());
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
    proc.last_pos_state = Some(crate::metadata::PlayState::Playing);
    proc.frame_count = 500;
    proc.strip_logged = true;
    proc.pending_meta_pic = Some(vec![1, 2, 3]);
    proc.ops.push_back(crate::proxy::FrameOp::Pass(100));
    proc.header_buf.extend_from_slice(&[0u8; 16]);

    proc.reset_for_start(DSD);

    assert!(proc.ops.is_empty());
    assert!(!proc.injected);
    assert!(proc.last_title.is_none());
    assert!(proc.last_pos_state.is_none());
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

    // 0x1D = PCM(0x01) | PIC(0x04) | META(0x08) | POS(0x10)
    let mut h = header(0x1D, 100, 50, 300, 5000);
    proc.strip(&mut h);

    // After strip: PCM(0x01) | POS(0x10) = 0x11
    assert_eq!(h.type_mask, 0x11);
}

fn pos_now(state: PlayState) -> PlaybackPosition {
    PlaybackPosition {
        length_seconds: 225,
        position_seconds: 10.0,
        captured_at: Instant::now(),
        state,
        track: 1,
        tracks_total: 10,
    }
}

#[test]
fn decide_pos_passthrough_when_no_position() {
    let proc = new_processor();
    assert_eq!(proc.decide_pos_action(None), PosAction::Passthrough);
}

#[test]
fn decide_pos_inject_on_cadence_tick() {
    let mut proc = new_processor();
    proc.frame_count = 20;
    proc.last_pos_state = Some(PlayState::Playing);
    let p = pos_now(PlayState::Playing);
    assert_eq!(proc.decide_pos_action(Some(&p)), PosAction::Inject);
}

#[test]
fn decide_pos_passthrough_between_cadence_ticks() {
    let mut proc = new_processor();
    proc.frame_count = 11;
    proc.last_pos_state = Some(PlayState::Playing);
    let p = pos_now(PlayState::Playing);
    assert_eq!(proc.decide_pos_action(Some(&p)), PosAction::Passthrough);
}

#[test]
fn decide_pos_inject_on_first_sight() {
    let mut proc = new_processor();
    proc.frame_count = 1;
    assert!(proc.last_pos_state.is_none());
    let p = pos_now(PlayState::Playing);
    assert_eq!(proc.decide_pos_action(Some(&p)), PosAction::Inject);
}

#[test]
fn decide_pos_inject_on_state_transition() {
    let mut proc = new_processor();
    proc.frame_count = 7;
    proc.last_pos_state = Some(PlayState::Playing);
    let p = pos_now(PlayState::Paused);
    assert_eq!(proc.decide_pos_action(Some(&p)), PosAction::Inject);
}

#[test]
fn decide_pos_paused_mid_cadence_passthrough() {
    let mut proc = new_processor();
    proc.frame_count = 7;
    proc.last_pos_state = Some(PlayState::Paused);
    let p = pos_now(PlayState::Paused);
    assert_eq!(proc.decide_pos_action(Some(&p)), PosAction::Passthrough);
}
