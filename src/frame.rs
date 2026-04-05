// src/frame.rs

pub const FRAME_HEADER_SIZE: usize = 32;
pub const TYPE_PCM: u32 = 0x01;
pub const TYPE_PIC: u32 = 0x04;
pub const TYPE_META: u32 = 0x08;
pub const TYPE_POS: u32 = 0x10;

pub struct FrameHeader {
    pub type_mask: u32,
    pub pcm_len: u32,
    pub pos_len: u32,
    pub meta_len: u32,
    pub pic_len: u32,
}

impl FrameHeader {
    pub fn has_meta(&self) -> bool {
        self.type_mask & TYPE_META != 0
    }

    pub fn has_pic(&self) -> bool {
        self.type_mask & TYPE_PIC != 0
    }
}

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
    Some(FrameHeader {
        type_mask: u32::from_le_bytes(buf[0..4].try_into().unwrap()),
        pcm_len: u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        pos_len: u32::from_le_bytes(buf[8..12].try_into().unwrap()),
        meta_len: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
        pic_len: u32::from_le_bytes(buf[16..20].try_into().unwrap()),
    })
}

/// Serialize a FrameHeader back to 32 bytes (for modified headers).
pub fn serialize_header(h: &FrameHeader) -> [u8; FRAME_HEADER_SIZE] {
    let mut buf = [0u8; FRAME_HEADER_SIZE];
    buf[0..4].copy_from_slice(&h.type_mask.to_le_bytes());
    buf[4..8].copy_from_slice(&h.pcm_len.to_le_bytes());
    buf[8..12].copy_from_slice(&h.pos_len.to_le_bytes());
    buf[12..16].copy_from_slice(&h.meta_len.to_le_bytes());
    buf[16..20].copy_from_slice(&h.pic_len.to_le_bytes());
    buf
}

/// Compute bytes_per_sample: max(1, bits / 8).
/// PCM (bits=32) → 4. DSD (bits=1) → 1.
pub fn bytes_per_sample(bits: u32) -> u32 {
    std::cmp::max(1, bits / 8)
}

/// Build the metadata section bytes for NAA v6.
/// Format: `[metadata]\n` + key=value lines + `\0`
///
/// `params` provides the format fields (bitrate, bits, channels, etc).
/// Only whitelisted fields — the NAA endpoint rejects unknown fields.
pub fn build_meta_section(
    params: &StreamParams,
    title: &str,
    artist: &str,
    album: &str,
) -> Vec<u8> {
    let meta_rate = if params.is_dsd { 2822400 } else { params.rate };
    let bitrate = meta_rate as u64 * params.bits as u64 * 2;
    let sdm = if params.is_dsd { 1 } else { 0 };
    let content = format!(
        "bitrate={}\nbits={}\nchannels=2\nfloat=0\nsamplerate={}\nsdm={}\nsong={}\nartist={}\nalbum={}\n",
        bitrate, params.bits, meta_rate, sdm, title, artist, album
    );
    let mut section = format!("[metadata]\n{}", content).into_bytes();
    section.push(0x00);
    section
}

/// Build a meta_template from stream parameters (used as default metadata).
/// This is what HQPlayer sends — format fields + `song=Roon`.
pub fn build_meta_template(params: &StreamParams) -> Vec<u8> {
    let meta_rate = if params.is_dsd { 2822400 } else { params.rate };
    let bitrate = meta_rate as u64 * params.bits as u64 * 2;
    let sdm = if params.is_dsd { 1 } else { 0 };
    format!(
        "bitrate={}\nbits={}\nchannels=2\nfloat=0\nsamplerate={}\nsdm={}\nsong=Roon\n",
        bitrate, params.bits, meta_rate, sdm
    )
    .into_bytes()
}

/// Parse an XML start message for stream parameters.
/// Extracts bits="N", rate="N", stream="pcm|dsd" from the XML text.
/// Returns None if this isn't a start message or lacks required attributes.
pub fn parse_start_message(xml: &[u8]) -> Option<StreamParams> {
    let text = std::str::from_utf8(xml).ok()?;
    if !text.contains("type=\"start\"") || text.contains("result=") {
        return None;
    }
    let bits = extract_xml_attr(text, "bits")?.parse::<u32>().ok()?;
    let rate = extract_xml_attr(text, "rate")?.parse::<u32>().ok()?;
    let stream = extract_xml_attr(text, "stream").unwrap_or("pcm".to_string());
    let is_dsd = stream == "dsd";
    Some(StreamParams {
        bits,
        rate,
        is_dsd,
        bytes_per_sample: bytes_per_sample(bits),
    })
}

/// Extract an XML attribute value: `name="value"` → `value`
fn extract_xml_attr(text: &str, name: &str) -> Option<String> {
    let pattern = format!("{}=\"", name);
    let start = text.find(&pattern)? + pattern.len();
    let end = text[start..].find('"')? + start;
    Some(text[start..end].to_string())
}

/// Returns true if the header looks corrupt (sanity check).
pub fn is_corrupt(header: &FrameHeader) -> bool {
    header.pcm_len > 1_000_000 || header.pos_len > 10_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_header_pcm() {
        let mut buf = [0u8; 32];
        buf[0..4].copy_from_slice(&0x1Du32.to_le_bytes());
        buf[4..8].copy_from_slice(&81920u32.to_le_bytes());
        buf[8..12].copy_from_slice(&271u32.to_le_bytes());
        buf[12..16].copy_from_slice(&100u32.to_le_bytes());
        buf[16..20].copy_from_slice(&5000u32.to_le_bytes());

        let h = parse_header(&buf).unwrap();
        assert_eq!(h.type_mask, 0x1D);
        assert_eq!(h.pcm_len, 81920);
        assert_eq!(h.pos_len, 271);
        assert_eq!(h.meta_len, 100);
        assert_eq!(h.pic_len, 5000);
        assert!(h.has_meta());
        assert!(h.has_pic());
    }

    #[test]
    fn test_parse_header_dsd() {
        let mut buf = [0u8; 32];
        buf[0..4].copy_from_slice(&0x11u32.to_le_bytes());
        buf[4..8].copy_from_slice(&602112u32.to_le_bytes());
        buf[8..12].copy_from_slice(&271u32.to_le_bytes());

        let h = parse_header(&buf).unwrap();
        assert_eq!(h.type_mask, 0x11);
        assert_eq!(h.pcm_len, 602112);
        assert!(!h.has_meta());
        assert!(!h.has_pic());
    }

    #[test]
    fn test_parse_header_too_short() {
        let buf = [0u8; 16];
        assert!(parse_header(&buf).is_none());
    }

    #[test]
    fn test_serialize_roundtrip() {
        let h = FrameHeader {
            type_mask: 0x1D,
            pcm_len: 81920,
            pos_len: 271,
            meta_len: 100,
            pic_len: 5000,
        };
        let buf = serialize_header(&h);
        let h2 = parse_header(&buf).unwrap();
        assert_eq!(h2.type_mask, 0x1D);
        assert_eq!(h2.pcm_len, 81920);
        assert_eq!(h2.pos_len, 271);
        assert_eq!(h2.meta_len, 100);
        assert_eq!(h2.pic_len, 5000);
    }

    #[test]
    fn test_bytes_per_sample_pcm() {
        assert_eq!(bytes_per_sample(32), 4);
        assert_eq!(bytes_per_sample(16), 2);
        assert_eq!(bytes_per_sample(24), 3);
    }

    #[test]
    fn test_bytes_per_sample_dsd() {
        assert_eq!(bytes_per_sample(1), 1);
    }

    #[test]
    fn test_build_meta_section_pcm() {
        let params = StreamParams {
            bits: 32,
            rate: 384000,
            is_dsd: false,
            bytes_per_sample: 4,
        };
        let section = build_meta_section(&params, "My Song", "My Artist", "My Album");
        let text = String::from_utf8_lossy(&section);
        assert!(text.starts_with("[metadata]\n"));
        assert!(text.contains("bitrate=24576000\n"));
        assert!(text.contains("bits=32\n"));
        assert!(text.contains("samplerate=384000\n"));
        assert!(text.contains("sdm=0\n"));
        assert!(text.contains("song=My Song\n"));
        assert!(text.contains("artist=My Artist\n"));
        assert!(text.contains("album=My Album\n"));
        assert!(section.last() == Some(&0x00));
    }

    #[test]
    fn test_build_meta_section_dsd() {
        let params = StreamParams {
            bits: 1,
            rate: 22579200,
            is_dsd: true,
            bytes_per_sample: 1,
        };
        let section = build_meta_section(&params, "DSD Track", "Artist", "Album");
        let text = String::from_utf8_lossy(&section);
        assert!(text.contains("bitrate=5644800\n"));
        assert!(text.contains("bits=1\n"));
        assert!(text.contains("samplerate=2822400\n"));
        assert!(text.contains("sdm=1\n"));
    }

    #[test]
    fn test_parse_start_message_pcm() {
        let xml = br#"<?xml version="1.0" encoding="utf-8"?><networkaudio><operation bits="32" channels="2" rate="384000" stream="pcm" type="start"/></networkaudio>"#;
        let params = parse_start_message(xml).unwrap();
        assert_eq!(params.bits, 32);
        assert_eq!(params.rate, 384000);
        assert!(!params.is_dsd);
        assert_eq!(params.bytes_per_sample, 4);
    }

    #[test]
    fn test_parse_start_message_dsd() {
        let xml = br#"<?xml version="1.0" encoding="utf-8"?><networkaudio><operation bits="1" channels="2" rate="22579200" stream="dsd" type="start"/></networkaudio>"#;
        let params = parse_start_message(xml).unwrap();
        assert_eq!(params.bits, 1);
        assert_eq!(params.rate, 22579200);
        assert!(params.is_dsd);
        assert_eq!(params.bytes_per_sample, 1);
    }

    #[test]
    fn test_parse_start_message_result_ignored() {
        let xml = br#"<networkaudio><operation result="1" type="start" rate="384000" bits="32"/></networkaudio>"#;
        assert!(parse_start_message(xml).is_none());
    }

    #[test]
    fn test_parse_start_message_not_start() {
        let xml = br#"<networkaudio><operation type="stop"/></networkaudio>"#;
        assert!(parse_start_message(xml).is_none());
    }

    #[test]
    fn test_is_corrupt() {
        let h = FrameHeader {
            type_mask: 0x1D,
            pcm_len: 2_000_000,
            pos_len: 271,
            meta_len: 0,
            pic_len: 0,
        };
        assert!(is_corrupt(&h));

        let h2 = FrameHeader {
            type_mask: 0x1D,
            pcm_len: 81920,
            pos_len: 20_000,
            meta_len: 0,
            pic_len: 0,
        };
        assert!(is_corrupt(&h2));

        let h3 = FrameHeader {
            type_mask: 0x11,
            pcm_len: 602112,
            pos_len: 271,
            meta_len: 0,
            pic_len: 0,
        };
        assert!(!is_corrupt(&h3));
    }
}
