use crate::frame::*;

fn header(type_mask: u32, pcm_len: u32, pos_len: u32, meta_len: u32, pic_len: u32) -> FrameHeader {
    FrameHeader {
        raw: [0u8; FRAME_HEADER_SIZE],
        type_mask,
        pcm_len,
        pos_len,
        meta_len,
        pic_len,
    }
}

#[test]
fn parse_header_pcm() {
    let mut buf = [0u8; 32];
    buf[0..4].copy_from_slice(&0x1Du32.to_le_bytes());
    buf[4..8].copy_from_slice(&81920u32.to_le_bytes());
    buf[8..12].copy_from_slice(&271u32.to_le_bytes());
    buf[12..16].copy_from_slice(&100u32.to_le_bytes());
    buf[16..20].copy_from_slice(&5000u32.to_le_bytes());

    let h = parse_header(&buf).unwrap();
    assert_eq!(h, header(0x1D, 81920, 271, 100, 5000));
    assert!(h.has_meta());
}

#[test]
fn parse_header_dsd() {
    let mut buf = [0u8; 32];
    buf[0..4].copy_from_slice(&0x11u32.to_le_bytes());
    buf[4..8].copy_from_slice(&602112u32.to_le_bytes());
    buf[8..12].copy_from_slice(&271u32.to_le_bytes());

    let h = parse_header(&buf).unwrap();
    assert_eq!(h.type_mask, 0x11);
    assert_eq!(h.pcm_len, 602112);
    assert!(!h.has_meta());
}

#[test]
fn parse_header_too_short() {
    assert!(parse_header(&[0u8; 16]).is_none());
}

#[test]
fn serialize_roundtrip() {
    let original = header(0x1D, 81920, 271, 100, 5000);
    let reparsed = parse_header(&serialize_header(&original)).unwrap();
    assert_eq!(reparsed, original);
}

#[test]
fn bytes_per_sample_pcm() {
    assert_eq!(bytes_per_sample(32), 4);
    assert_eq!(bytes_per_sample(24), 3);
    assert_eq!(bytes_per_sample(16), 2);
}

#[test]
fn bytes_per_sample_dsd() {
    assert_eq!(bytes_per_sample(1), 1);
}

#[test]
fn meta_section_pcm() {
    let params = StreamParams { bits: 32, rate: 384000, is_dsd: false, bytes_per_sample: 4 };
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
    assert_eq!(section.last(), Some(&0x00));
}

#[test]
fn meta_section_dsd_uses_base_rate() {
    let params = StreamParams { bits: 1, rate: 22579200, is_dsd: true, bytes_per_sample: 1 };
    let section = build_meta_section(&params, "DSD Track", "Artist", "Album");
    let text = String::from_utf8_lossy(&section);
    assert!(text.contains("bitrate=5644800\n"));
    assert!(text.contains("bits=1\n"));
    assert!(text.contains("samplerate=2822400\n"));
    assert!(text.contains("sdm=1\n"));
}

#[test]
fn parse_start_pcm() {
    let xml = br#"<?xml version="1.0" encoding="utf-8"?><networkaudio><operation bits="32" channels="2" rate="384000" stream="pcm" type="start"/></networkaudio>"#;
    let params = parse_start_message(xml).unwrap();
    assert_eq!(params, StreamParams { bits: 32, rate: 384000, is_dsd: false, bytes_per_sample: 4 });
}

#[test]
fn parse_start_dsd() {
    let xml = br#"<?xml version="1.0" encoding="utf-8"?><networkaudio><operation bits="1" channels="2" rate="22579200" stream="dsd" type="start"/></networkaudio>"#;
    let params = parse_start_message(xml).unwrap();
    assert_eq!(params, StreamParams { bits: 1, rate: 22579200, is_dsd: true, bytes_per_sample: 1 });
}

#[test]
fn parse_start_ignores_result_messages() {
    let xml = br#"<networkaudio><operation result="1" type="start" rate="384000" bits="32"/></networkaudio>"#;
    assert!(parse_start_message(xml).is_none());
}

#[test]
fn parse_start_ignores_non_start() {
    let xml = br#"<networkaudio><operation type="stop"/></networkaudio>"#;
    assert!(parse_start_message(xml).is_none());
}

#[test]
fn corrupt_pcm_len() {
    assert!(is_corrupt(&header(0x1D, 2_000_000, 271, 0, 0)));
}

#[test]
fn corrupt_pos_len() {
    assert!(is_corrupt(&header(0x1D, 81920, 20_000, 0, 0)));
}

#[test]
fn corrupt_meta_len() {
    assert!(is_corrupt(&header(0x1D, 81920, 271, 200_000, 0)));
}

#[test]
fn corrupt_pic_len() {
    assert!(is_corrupt(&header(0x1D, 81920, 271, 0, 2_000_000)));
}

#[test]
fn not_corrupt_normal_frame() {
    assert!(!is_corrupt(&header(0x11, 602112, 271, 0, 0)));
}
