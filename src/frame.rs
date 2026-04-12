pub const FRAME_HEADER_SIZE: usize = 32;
pub const TYPE_PIC: u32 = 0x04;
pub const TYPE_META: u32 = 0x08;

#[derive(Debug)]
pub struct FrameHeader {
    pub raw: [u8; FRAME_HEADER_SIZE],
    pub type_mask: u32,
    pub pcm_len: u32,
    pub pos_len: u32,
    pub meta_len: u32,
    pub pic_len: u32,
}

impl PartialEq for FrameHeader {
    fn eq(&self, other: &Self) -> bool {
        self.type_mask == other.type_mask
            && self.pcm_len == other.pcm_len
            && self.pos_len == other.pos_len
            && self.meta_len == other.meta_len
            && self.pic_len == other.pic_len
    }
}

impl FrameHeader {
    pub fn has_meta(&self) -> bool {
        self.type_mask & TYPE_META != 0
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub struct StreamParams {
    pub bits: u32,
    pub rate: u32,
    pub is_dsd: bool,
    pub bytes_per_sample: u32,
}

/// Parse a 32-byte frame header. Returns None if buffer is too short.
pub fn parse_header(buf: &[u8]) -> Option<FrameHeader> {
    if buf.len() < FRAME_HEADER_SIZE {
        return None;
    }
    let mut raw = [0u8; FRAME_HEADER_SIZE];
    raw.copy_from_slice(&buf[..FRAME_HEADER_SIZE]);
    Some(FrameHeader {
        raw,
        type_mask: u32::from_le_bytes(buf[0..4].try_into().unwrap()),
        pcm_len: u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        pos_len: u32::from_le_bytes(buf[8..12].try_into().unwrap()),
        meta_len: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
        pic_len: u32::from_le_bytes(buf[16..20].try_into().unwrap()),
    })
}

/// Serialize a FrameHeader back to 32 bytes.
/// Writes modified fields back into the preserved raw header.
pub fn serialize_header(h: &FrameHeader) -> [u8; FRAME_HEADER_SIZE] {
    let mut buf = h.raw;
    buf[0..4].copy_from_slice(&h.type_mask.to_le_bytes());
    buf[4..8].copy_from_slice(&h.pcm_len.to_le_bytes());
    buf[8..12].copy_from_slice(&h.pos_len.to_le_bytes());
    buf[12..16].copy_from_slice(&h.meta_len.to_le_bytes());
    buf[16..20].copy_from_slice(&h.pic_len.to_le_bytes());
    buf
}

/// Compute bytes_per_sample: max(1, bits / 8).
/// PCM (bits=32) -> 4. DSD (bits=1) -> 1.
pub fn bytes_per_sample(bits: u32) -> u32 {
    std::cmp::max(1, bits / 8)
}

/// Build the metadata section bytes for NAA v6.
/// Format: `[metadata]\n` + key=value lines + `\0`
///
/// Only whitelisted fields -- the NAA endpoint rejects unknown fields.
pub fn build_meta_section(
    params: &StreamParams,
    title: &str,
    artist: &str,
    album: &str,
) -> Vec<u8> {
    use std::io::Write;
    let meta_rate = if params.is_dsd { 2822400 } else { params.rate };
    let bitrate = meta_rate as u64 * params.bits as u64 * 2;
    let sdm = u32::from(params.is_dsd);
    let mut section = Vec::with_capacity(256);
    write!(
        section,
        "[metadata]\nbitrate={}\nbits={}\nchannels=2\nfloat=0\nsamplerate={}\nsdm={}\nsong={}\nartist={}\nalbum={}\n",
        bitrate, params.bits, meta_rate, sdm, title, artist, album,
    )
    .unwrap();
    section.push(0x00);
    section
}

/// Parse an XML start message for stream parameters.
/// Returns None if this isn't a start message or lacks required attributes.
pub fn parse_start_message(xml: &[u8]) -> Option<StreamParams> {
    let text = std::str::from_utf8(xml).ok()?;
    if !text.contains("type=\"start\"") || text.contains("result=") {
        return None;
    }
    let bits = extract_xml_attr(text, "bits")?.parse::<u32>().ok()?;
    let rate = extract_xml_attr(text, "rate")?.parse::<u32>().ok()?;
    let is_dsd = extract_xml_attr(text, "stream").as_deref() == Some("dsd");
    Some(StreamParams {
        bits,
        rate,
        is_dsd,
        bytes_per_sample: bytes_per_sample(bits),
    })
}

/// Extract an XML attribute value: ` name="value"` -> `value`.
/// Space prefix prevents matching attribute name suffixes
/// (e.g., searching for "rate" won't false-match "bitrate").
fn extract_xml_attr(text: &str, name: &str) -> Option<String> {
    let pattern = format!(" {}=\"", name);
    let start = text.find(&pattern)? + pattern.len();
    let end = text[start..].find('"')? + start;
    Some(text[start..end].to_string())
}

/// Returns true if the header looks corrupt (sanity check).
/// Guards against garbage lengths that would desync the state machine.
pub fn is_corrupt(header: &FrameHeader) -> bool {
    header.pcm_len > 1_000_000
        || header.pos_len > 10_000
        || header.meta_len > 100_000
        || header.pic_len > 1_000_000
}

use crate::metadata::{PlayState, PlaybackPosition};
use std::time::Instant;

/// Build a NAA `[position]` section matching HQPlayer's byte layout.
///
/// Format: `[position]\n` + `key=value\n` lines + `\0`.
/// 13 fields in HQPlayer's emitted order — unknown or reordered fields
/// cause the T8's whitelist parser to reject the whole section.
pub fn build_pos_section(pos: &PlaybackPosition, now: Instant) -> Vec<u8> {
    use std::io::Write;

    let elapsed = if matches!(pos.state, PlayState::Playing) {
        now.saturating_duration_since(pos.captured_at).as_secs_f64()
    } else {
        0.0
    };
    let length_f = f64::from(pos.length_seconds);
    let effective_pos = (pos.position_seconds + elapsed).clamp(0.0, length_f);

    let total_secs = pos.length_seconds;
    let total_min = total_secs / 60;
    let total_sec = total_secs % 60;

    let remain = (length_f - effective_pos).max(0.0) as u32;
    let remain_min = remain / 60;
    let remain_sec = remain % 60;

    let begin = effective_pos as u32;
    let begin_min = begin / 60;
    let begin_sec = begin % 60;

    let state_str = match pos.state {
        PlayState::Playing => "PLAYING",
        PlayState::Paused => "PAUSED",
    };

    let mut section = Vec::with_capacity(320);
    write!(
        section,
        "[position]\n\
         apod=0\n\
         begin_min={begin_min}\n\
         begin_sec={begin_sec}\n\
         clips=0\n\
         correction=0\n\
         input_fill=-1.00000000000000000\n\
         length={length_f:.17}\n\
         output_fill=0.00000000000000000\n\
         position={effective_pos:.17}\n\
         remain_min={remain_min}\n\
         remain_sec={remain_sec}\n\
         state={state_str}\n\
         total_min={total_min}\n\
         total_sec={total_sec}\n\
         track={track}\n\
         tracks_total={tracks_total}\n",
        track = pos.track,
        tracks_total = pos.tracks_total,
    )
    .unwrap();
    section.push(0x00);
    section
}
