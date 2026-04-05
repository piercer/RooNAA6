use std::io::{self, Read, Write};
use std::net::TcpStream;

use crate::frame::{
    build_meta_section, is_corrupt, parse_header, parse_start_message, serialize_header,
    FrameHeader, StreamParams, FRAME_HEADER_SIZE, TYPE_META, TYPE_PIC,
};
use crate::metadata::{Metadata, SharedMetadata};
use crate::ts;

/// Forward NAA->HQP: simple byte passthrough with XML logging.
pub fn forward_passthrough(mut src: TcpStream, mut dst: TcpStream, label: &str) {
    let mut buf = [0u8; 65536];
    loop {
        match src.read(&mut buf) {
            Ok(0) => {
                eprintln!("{} [{}] EOF", ts(), label);
                break;
            }
            Ok(n) => {
                if buf[0] == b'<' {
                    log_xml(label, &buf[..n]);
                }
                if let Err(e) = dst.write_all(&buf[..n]) {
                    eprintln!("{} [{}] write error: {}", ts(), label, e);
                    break;
                }
            }
            Err(e) => {
                eprintln!("{} [{}] read error: {}", ts(), label, e);
                break;
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum Phase {
    Header,
    Pass,
    Skip,
}

struct FrameProcessor {
    shared: SharedMetadata,
    params: StreamParams,
    phase: Phase,
    pass_remaining: usize,
    skip_remaining: usize,
    pending_inject: Option<Vec<u8>>,
    header_buf: Vec<u8>,
    injected: bool,
    last_title: Option<String>,
    frame_count: u64,
    strip_logged: bool,
}

impl FrameProcessor {
    fn new(shared: SharedMetadata) -> Self {
        Self {
            shared,
            params: StreamParams {
                bits: 32,
                rate: 44100,
                is_dsd: false,
                bytes_per_sample: 4,
            },
            phase: Phase::Header,
            pass_remaining: 0,
            skip_remaining: 0,
            pending_inject: None,
            header_buf: Vec::with_capacity(FRAME_HEADER_SIZE),
            injected: false,
            last_title: None,
            frame_count: 0,
            strip_logged: false,
        }
    }

    fn reset_for_start(&mut self, params: StreamParams) {
        self.params = params;
        self.phase = Phase::Header;
        self.injected = false;
        self.last_title = None;
        self.frame_count = 0;
        self.strip_logged = false;
        self.header_buf.clear();
        self.pass_remaining = 0;
        self.skip_remaining = 0;
        self.pending_inject = None;
    }

    /// Build and set the injection payload. Returns JPEG size for logging.
    fn inject(&mut self, header: &mut FrameHeader, meta: &Metadata, with_cover: bool) -> usize {
        let meta_section =
            build_meta_section(&self.params, &meta.title, &meta.artist, &meta.album);
        header.type_mask |= TYPE_META;
        header.meta_len = meta_section.len() as u32;

        let mut payload = meta_section;
        let mut jpeg_len = 0;
        if with_cover {
            if let Some(jpeg) = &meta.cover_art {
                jpeg_len = jpeg.len();
                header.type_mask |= TYPE_PIC;
                header.pic_len = jpeg_len as u32;
                payload.extend_from_slice(jpeg);
            }
        }
        self.pending_inject = Some(payload);
        self.last_title = Some(meta.title.clone());
        jpeg_len
    }

    fn strip(&mut self, header: &mut FrameHeader) {
        header.type_mask &= !(TYPE_META | TYPE_PIC);
        header.meta_len = 0;
        header.pic_len = 0;
        self.pending_inject = None;
    }
}

/// Handle XML data: log it, check for start messages, reset state, forward to dst.
fn handle_xml(
    proc: &mut FrameProcessor,
    data: &[u8],
    dst: &mut TcpStream,
    label: &str,
) -> io::Result<()> {
    log_xml(label, data);
    if let Some(params) = parse_start_message(data) {
        eprintln!(
            "{} [{}] start: {} bytes/sample, {} {}Hz",
            ts(),
            label,
            params.bytes_per_sample,
            if params.is_dsd { "dsd" } else { "pcm" },
            params.rate,
        );
        proc.reset_for_start(params);
    }
    dst.write_all(data)
}

/// Forward HQP->NAA: frame-level processing with metadata injection.
///
/// State machine processes NAA v6 binary frames, injecting Roon metadata
/// and cover art into the audio stream so the DAC displays track info.
///
/// Actions:
/// - INJECT: first META frame after start -- inject title/artist/album + cover art
/// - GAPLESS: track change during gapless playback -- inject new metadata + cover
/// - STRIP: HQPlayer sends its own META refresh -- strip it to keep our metadata
/// - REFRESH: periodic re-injection (~every 300 frames / ~30s) to prevent DAC revert
/// - PASSTHROUGH: normal frames with no metadata work needed
pub fn forward_hqp_to_naa(mut src: TcpStream, mut dst: TcpStream, shared: SharedMetadata) {
    let label = "HQP->NAA";
    let mut buf = [0u8; 65536];
    let mut proc = FrameProcessor::new(shared);
    let mut out = Vec::with_capacity(65536 + 4096);

    loop {
        let n = match src.read(&mut buf) {
            Ok(0) => {
                eprintln!("{} [{}] EOF", ts(), label);
                break;
            }
            Ok(n) => n,
            Err(e) => {
                eprintln!("{} [{}] read error: {}", ts(), label, e);
                break;
            }
        };
        let data = &buf[..n];

        // Top-of-buffer XML check (before binary frame processing)
        if proc.phase == Phase::Header && proc.header_buf.is_empty() {
            if let Some(idx) = data.iter().position(|&b| !b.is_ascii_whitespace()) {
                if data[idx] == b'<' {
                    if let Err(e) = handle_xml(&mut proc, data, &mut dst, label) {
                        eprintln!("{} [{}] write error: {}", ts(), label, e);
                        break;
                    }
                    continue;
                }
            }
        }

        // Binary frame processing
        let mut pos = 0;
        out.clear();

        while pos < data.len() {
            match proc.phase {
                Phase::Header => {
                    // Mid-buffer XML check
                    if proc.header_buf.is_empty() && data[pos] == b'<' {
                        if !out.is_empty() {
                            if let Err(e) = dst.write_all(&out) {
                                eprintln!("{} [{}] write error: {}", ts(), label, e);
                                return;
                            }
                            out.clear();
                        }
                        if let Err(e) = handle_xml(&mut proc, &data[pos..], &mut dst, label) {
                            eprintln!("{} [{}] write error: {}", ts(), label, e);
                            return;
                        }
                        break; // rest of buffer is XML, already sent
                    }

                    // Accumulate header bytes (32 total)
                    let need = FRAME_HEADER_SIZE - proc.header_buf.len();
                    let take = need.min(data.len() - pos);
                    proc.header_buf.extend_from_slice(&data[pos..pos + take]);
                    pos += take;

                    if proc.header_buf.len() < FRAME_HEADER_SIZE {
                        continue;
                    }

                    let mut header = parse_header(&proc.header_buf)
                        .expect("header_buf is FRAME_HEADER_SIZE");
                    proc.header_buf.clear();

                    if is_corrupt(&header) {
                        eprintln!(
                            "{} [CORRUPT] pcm_len={} pos_len={} meta_len={} pic_len={}, closing",
                            ts(),
                            header.pcm_len,
                            header.pos_len,
                            header.meta_len,
                            header.pic_len,
                        );
                        return;
                    }

                    let pcm_bytes =
                        header.pcm_len as usize * proc.params.bytes_per_sample as usize;
                    let pos_bytes = header.pos_len as usize;
                    let orig_meta_len = header.meta_len as usize;
                    let orig_pic_len = header.pic_len as usize;
                    let has_meta = header.has_meta();

                    let meta = proc.shared.get();
                    let title = &meta.title;
                    proc.frame_count += 1;

                    // Decide action; replace_original means we strip the original meta/pic
                    let replace_original =
                        if !title.is_empty() && has_meta && !proc.injected {
                            // INJECT: first META frame after start
                            let jpeg_len = proc.inject(&mut header, &meta, true);
                            proc.injected = true;
                            eprintln!(
                                "{} [INJECT] {} / {} / {} + {}b cover",
                                ts(), title, meta.artist, meta.album, jpeg_len,
                            );
                            true
                        } else if !title.is_empty()
                            && proc.injected
                            && proc.last_title.as_deref() != Some(title)
                        {
                            // GAPLESS: track changed during gapless playback
                            let jpeg_len = proc.inject(&mut header, &meta, true);
                            eprintln!(
                                "{} [GAPLESS] {} / {} / {} + {}b cover",
                                ts(), title, meta.artist, meta.album, jpeg_len,
                            );
                            true
                        } else if has_meta && proc.injected {
                            // STRIP: HQPlayer META refresh -- strip to keep our metadata
                            proc.strip(&mut header);
                            if !proc.strip_logged {
                                eprintln!(
                                    "{} [STRIP] META refresh stripped (frame {})",
                                    ts(),
                                    proc.frame_count,
                                );
                                proc.strip_logged = true;
                            }
                            true
                        } else if proc.injected
                            && !title.is_empty()
                            && proc.frame_count % 300 == 0
                        {
                            // REFRESH: periodic re-injection (~30s)
                            proc.inject(&mut header, &meta, false);
                            eprintln!(
                                "{} [REFRESH] {} (frame {})",
                                ts(), title, proc.frame_count,
                            );
                            true
                        } else {
                            // PASSTHROUGH: no metadata work needed
                            proc.pending_inject = None;
                            false
                        };

                    if replace_original {
                        proc.pass_remaining = pcm_bytes + pos_bytes;
                        proc.skip_remaining = orig_meta_len + orig_pic_len;
                    } else {
                        proc.pass_remaining =
                            pcm_bytes + pos_bytes + orig_meta_len + orig_pic_len;
                        proc.skip_remaining = 0;
                    }

                    out.extend_from_slice(&serialize_header(&header));
                    proc.phase = Phase::Pass;
                }

                Phase::Pass => {
                    let take = proc.pass_remaining.min(data.len() - pos);
                    out.extend_from_slice(&data[pos..pos + take]);
                    pos += take;
                    proc.pass_remaining -= take;

                    if proc.pass_remaining == 0 {
                        if let Some(inject) = proc.pending_inject.take() {
                            out.extend_from_slice(&inject);
                        }
                        proc.phase = if proc.skip_remaining > 0 {
                            Phase::Skip
                        } else {
                            Phase::Header
                        };
                    }
                }

                Phase::Skip => {
                    let take = proc.skip_remaining.min(data.len() - pos);
                    pos += take; // discard bytes
                    proc.skip_remaining -= take;

                    if proc.skip_remaining == 0 {
                        proc.phase = Phase::Header;
                    }
                }
            }
        }

        if !out.is_empty() {
            if let Err(e) = dst.write_all(&out) {
                eprintln!("{} [{}] write error: {}", ts(), label, e);
                break;
            }
        }
    }
}

/// Log XML messages (skip keepalive).
pub fn log_xml(label: &str, data: &[u8]) {
    if data.is_empty() || data[0] != b'<' {
        return;
    }
    if data.windows(9).any(|w| w == b"keepalive") {
        return;
    }
    if let Ok(text) = std::str::from_utf8(data) {
        eprintln!("{} [{}] {}", ts(), label, text.trim());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{StreamParams, FRAME_HEADER_SIZE, TYPE_META, TYPE_PIC};
    use crate::metadata::{Metadata, SharedMetadata};
    use std::sync::Arc;

    fn pcm_params() -> StreamParams {
        StreamParams {
            bits: 32,
            rate: 44100,
            is_dsd: false,
            bytes_per_sample: 4,
        }
    }

    fn dsd_params() -> StreamParams {
        StreamParams {
            bits: 1,
            rate: 2822400,
            is_dsd: true,
            bytes_per_sample: 1,
        }
    }

    fn make_header(type_mask: u32, pcm_len: u32, pos_len: u32, meta_len: u32, pic_len: u32) -> FrameHeader {
        FrameHeader {
            raw: [0u8; FRAME_HEADER_SIZE],
            type_mask,
            pcm_len,
            pos_len,
            meta_len,
            pic_len,
        }
    }

    fn make_meta(title: &str, artist: &str, album: &str, cover: Option<&[u8]>) -> Metadata {
        Metadata {
            title: title.to_string(),
            artist: artist.to_string(),
            album: album.to_string(),
            cover_art: cover.map(|c| Arc::new(c.to_vec())),
        }
    }

    // --- new() defaults ---

    #[test]
    fn test_new_defaults() {
        let shared = SharedMetadata::new();
        let proc = FrameProcessor::new(shared);
        assert_eq!(proc.phase, Phase::Header);
        assert_eq!(proc.frame_count, 0);
        assert!(!proc.injected);
        assert!(proc.last_title.is_none());
        assert!(proc.pending_inject.is_none());
        assert_eq!(proc.pass_remaining, 0);
        assert_eq!(proc.skip_remaining, 0);
        assert!(!proc.strip_logged);
        assert_eq!(proc.params.bits, 32);
        assert!(!proc.params.is_dsd);
    }

    // --- reset_for_start() ---

    #[test]
    fn test_reset_clears_state() {
        let shared = SharedMetadata::new();
        let mut proc = FrameProcessor::new(shared);

        // Dirty up the state
        proc.injected = true;
        proc.last_title = Some("Old Song".into());
        proc.frame_count = 500;
        proc.strip_logged = true;
        proc.phase = Phase::Skip;
        proc.pass_remaining = 1000;
        proc.skip_remaining = 200;
        proc.pending_inject = Some(vec![1, 2, 3]);
        proc.header_buf.extend_from_slice(&[0u8; 16]);

        proc.reset_for_start(dsd_params());

        assert_eq!(proc.phase, Phase::Header);
        assert!(!proc.injected);
        assert!(proc.last_title.is_none());
        assert_eq!(proc.frame_count, 0);
        assert!(!proc.strip_logged);
        assert!(proc.header_buf.is_empty());
        assert_eq!(proc.pass_remaining, 0);
        assert_eq!(proc.skip_remaining, 0);
        assert!(proc.pending_inject.is_none());
        assert!(proc.params.is_dsd);
        assert_eq!(proc.params.bits, 1);
    }

    // --- inject() ---

    #[test]
    fn test_inject_with_cover() {
        let shared = SharedMetadata::new();
        let mut proc = FrameProcessor::new(shared);
        proc.params = pcm_params();

        let fake_jpeg = vec![0xFF, 0xD8, 0x00, 0x01];
        let meta = make_meta("Title", "Artist", "Album", Some(&fake_jpeg));
        let mut header = make_header(0x01, 100, 50, 0, 0);

        let jpeg_len = proc.inject(&mut header, &meta, true);

        assert_eq!(jpeg_len, 4);
        assert!(header.type_mask & TYPE_META != 0);
        assert!(header.type_mask & TYPE_PIC != 0);
        assert!(header.meta_len > 0);
        assert_eq!(header.pic_len, 4);
        assert!(proc.pending_inject.is_some());
        assert_eq!(proc.last_title.as_deref(), Some("Title"));

        // Payload should contain meta section + JPEG
        let payload = proc.pending_inject.unwrap();
        assert!(payload.ends_with(&fake_jpeg));
    }

    #[test]
    fn test_inject_without_cover() {
        let shared = SharedMetadata::new();
        let mut proc = FrameProcessor::new(shared);
        proc.params = pcm_params();

        let meta = make_meta("Title", "Artist", "Album", Some(&[0xFF, 0xD8]));
        let mut header = make_header(0x01, 100, 50, 0, 0);

        let jpeg_len = proc.inject(&mut header, &meta, false);

        assert_eq!(jpeg_len, 0);
        assert!(header.type_mask & TYPE_META != 0);
        assert_eq!(header.type_mask & TYPE_PIC, 0);
        assert_eq!(header.pic_len, 0);
        assert!(proc.pending_inject.is_some());

        // Payload should be meta section only, no JPEG
        let payload = proc.pending_inject.unwrap();
        assert!(!payload.windows(2).any(|w| w == [0xFF, 0xD8]));
    }

    #[test]
    fn test_inject_no_cover_art_available() {
        let shared = SharedMetadata::new();
        let mut proc = FrameProcessor::new(shared);
        proc.params = pcm_params();

        let meta = make_meta("Title", "Artist", "Album", None);
        let mut header = make_header(0x01, 100, 50, 0, 0);

        let jpeg_len = proc.inject(&mut header, &meta, true);

        assert_eq!(jpeg_len, 0);
        assert!(header.type_mask & TYPE_META != 0);
        assert_eq!(header.type_mask & TYPE_PIC, 0);
        assert_eq!(header.pic_len, 0);
    }

    #[test]
    fn test_inject_meta_section_content() {
        let shared = SharedMetadata::new();
        let mut proc = FrameProcessor::new(shared);
        proc.params = pcm_params();

        let meta = make_meta("My Song", "My Artist", "My Album", None);
        let mut header = make_header(0x09, 100, 50, 200, 0);

        proc.inject(&mut header, &meta, false);

        let payload = proc.pending_inject.unwrap();
        let text = std::str::from_utf8(&payload[..payload.len() - 1]).unwrap();
        assert!(text.starts_with("[metadata]\n"));
        assert!(text.contains("song=My Song\n"));
        assert!(text.contains("artist=My Artist\n"));
        assert!(text.contains("album=My Album\n"));
        assert!(text.contains("samplerate=44100\n"));
        assert!(text.contains("sdm=0\n"));
    }

    #[test]
    fn test_inject_dsd_uses_base_rate() {
        let shared = SharedMetadata::new();
        let mut proc = FrameProcessor::new(shared);
        proc.params = dsd_params();

        let meta = make_meta("DSD Track", "Artist", "Album", None);
        let mut header = make_header(0x09, 100, 50, 200, 0);

        proc.inject(&mut header, &meta, false);

        let payload = proc.pending_inject.unwrap();
        let text = std::str::from_utf8(&payload[..payload.len() - 1]).unwrap();
        assert!(text.contains("samplerate=2822400\n"));
        assert!(text.contains("sdm=1\n"));
        assert!(text.contains("bits=1\n"));
    }

    // --- strip() ---

    #[test]
    fn test_strip_clears_meta_and_pic() {
        let shared = SharedMetadata::new();
        let mut proc = FrameProcessor::new(shared);
        proc.pending_inject = Some(vec![1, 2, 3]);

        let mut header = make_header(0x0D, 100, 50, 300, 5000);

        proc.strip(&mut header);

        assert_eq!(header.type_mask & TYPE_META, 0);
        assert_eq!(header.type_mask & TYPE_PIC, 0);
        assert_eq!(header.meta_len, 0);
        assert_eq!(header.pic_len, 0);
        assert!(proc.pending_inject.is_none());
    }

    #[test]
    fn test_strip_preserves_other_type_bits() {
        let shared = SharedMetadata::new();
        let mut proc = FrameProcessor::new(shared);

        let mut header = make_header(0x1D, 100, 50, 300, 5000);
        proc.strip(&mut header);

        // 0x1D = PCM(0x01) | PIC(0x04) | META(0x08) | POS(0x10)
        // After strip: PCM(0x01) | POS(0x10) = 0x11
        assert_eq!(header.type_mask, 0x11);
    }
}
