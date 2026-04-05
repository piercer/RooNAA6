use std::io::{Read, Write};
use std::net::TcpStream;

use crate::frame::{
    build_meta_section, is_corrupt, parse_header, parse_start_message, serialize_header,
    StreamParams, FRAME_HEADER_SIZE, TYPE_META, TYPE_PIC,
};
use crate::metadata::SharedMetadata;
use crate::ts;

/// Forward NAA→HQP: simple byte passthrough with XML logging.
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

#[derive(PartialEq)]
enum Phase {
    Header,
    Pass,
    Skip,
}

/// Forward HQP→NAA: frame-level processing with metadata injection.
///
/// State machine processes NAA v6 binary frames, injecting Roon metadata
/// and cover art into the audio stream so the T8 DAC displays track info.
///
/// Actions:
/// - INJECT: first META frame after start — inject title/artist/album + cover art
/// - GAPLESS: track change during gapless playback — inject new metadata + cover
/// - STRIP: HQPlayer sends its own META refresh — strip it to keep our metadata
/// - REFRESH: periodic re-injection (~every 300 frames / ~30s) to prevent T8 revert
/// - PASSTHROUGH: normal frames with no metadata work needed
pub fn forward_hqp_to_naa(mut src: TcpStream, mut dst: TcpStream, shared: SharedMetadata) {
    let label = "HQP->NAA";
    let mut buf = [0u8; 65536];

    let mut phase = Phase::Header;
    let mut pass_remaining: usize = 0;
    let mut skip_remaining: usize = 0;
    let mut pending_inject: Option<Vec<u8>> = None;
    let mut header_buf = Vec::with_capacity(FRAME_HEADER_SIZE);
    let mut stream_params = StreamParams {
        bits: 32,
        rate: 44100,
        is_dsd: false,
        bytes_per_sample: 4,
    };
    let mut injected_this_start = false;
    let mut last_injected_title: Option<String> = None;
    let mut frame_count: u64 = 0;
    let mut strip_logged = false;
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

        // Top-of-buffer XML check (before binary processing)
        if phase == Phase::Header && header_buf.is_empty() {
            // Find first non-whitespace byte
            let first_non_ws = data.iter().position(|&b| !b.is_ascii_whitespace());
            if let Some(idx) = first_non_ws {
                if data[idx] == b'<' {
                    log_xml(label, data);
                    if let Some(params) = parse_start_message(data) {
                        eprintln!(
                            "{} [{}] start: {} bytes/sample, {} {}Hz",
                            ts(),
                            label,
                            params.bytes_per_sample,
                            if params.is_dsd { "dsd" } else { "pcm" },
                            params.rate
                        );
                        stream_params = params;
                        injected_this_start = false;
                        last_injected_title = None;
                        frame_count = 0;
                        strip_logged = false;
                        header_buf.clear();
                        pass_remaining = 0;
                        skip_remaining = 0;
                        pending_inject = None;
                    }
                    if let Err(e) = dst.write_all(data) {
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
            match phase {
                Phase::Header => {
                    // Mid-buffer XML check
                    if header_buf.is_empty() && data[pos] == b'<' {
                        // Flush accumulated output
                        if !out.is_empty() {
                            if let Err(e) = dst.write_all(&out) {
                                eprintln!("{} [{}] write error: {}", ts(), label, e);
                                return;
                            }
                            out.clear();
                        }
                        let xml_data = &data[pos..];
                        log_xml(label, xml_data);
                        if let Some(params) = parse_start_message(xml_data) {
                            eprintln!(
                                "{} [{}] start: {} bytes/sample, {} {}Hz",
                                ts(),
                                label,
                                params.bytes_per_sample,
                                if params.is_dsd { "dsd" } else { "pcm" },
                                params.rate
                            );
                            stream_params = params;
                            injected_this_start = false;
                            last_injected_title = None;
                            frame_count = 0;
                            strip_logged = false;
                            header_buf.clear();
                            pass_remaining = 0;
                            skip_remaining = 0;
                            pending_inject = None;
                        }
                        if let Err(e) = dst.write_all(xml_data) {
                            eprintln!("{} [{}] write error: {}", ts(), label, e);
                            return;
                        }
                        break; // rest of buffer is XML, already sent
                    }

                    // Accumulate header bytes (32 total)
                    let need = FRAME_HEADER_SIZE - header_buf.len();
                    let available = data.len() - pos;
                    let take = std::cmp::min(need, available);
                    header_buf.extend_from_slice(&data[pos..pos + take]);
                    pos += take;

                    if header_buf.len() == FRAME_HEADER_SIZE {
                        let mut header = parse_header(&header_buf).expect("header_buf is FRAME_HEADER_SIZE");
                        header_buf.clear();

                        if is_corrupt(&header) {
                            eprintln!(
                                "{} [CORRUPT] pcm_len={} pos_len={}",
                                ts(),
                                header.pcm_len,
                                header.pos_len
                            );
                        }

                        let pcm_bytes =
                            header.pcm_len as usize * stream_params.bytes_per_sample as usize;
                        let has_meta = header.has_meta();

                        let meta = shared.get();
                        let title = &meta.title;
                        frame_count += 1;

                        // Save original lengths BEFORE any modification
                        let orig_meta_len = header.meta_len as usize;
                        let orig_pic_len = header.pic_len as usize;

                        if !title.is_empty() && has_meta && !injected_this_start {
                            // INJECT: first META frame after start
                            let meta_section = build_meta_section(
                                &stream_params,
                                title,
                                &meta.artist,
                                &meta.album,
                            );
                            let jpeg = shared.get_cover_art();
                            let jpeg_len = jpeg.as_ref().map_or(0, |j| j.len());

                            header.type_mask |= TYPE_META;
                            if jpeg.is_some() {
                                header.type_mask |= TYPE_PIC;
                            }
                            header.meta_len = meta_section.len() as u32;
                            header.pic_len = jpeg_len as u32;

                            let mut inject = meta_section;
                            if let Some(j) = jpeg {
                                inject.extend_from_slice(&j);
                            }
                            pending_inject = Some(inject);
                            injected_this_start = true;
                            last_injected_title = Some(title.clone());
                            pass_remaining = pcm_bytes + header.pos_len as usize;
                            skip_remaining = orig_meta_len + orig_pic_len;

                            eprintln!(
                                "{} [INJECT] {} / {} / {} + {}b cover",
                                ts(),
                                title,
                                meta.artist,
                                meta.album,
                                jpeg_len
                            );
                        } else if !title.is_empty()
                            && injected_this_start
                            && last_injected_title.as_deref() != Some(title)
                        {
                            // GAPLESS: track changed during gapless playback
                            let meta_section = build_meta_section(
                                &stream_params,
                                title,
                                &meta.artist,
                                &meta.album,
                            );
                            let jpeg = shared.get_cover_art();
                            let jpeg_len = jpeg.as_ref().map_or(0, |j| j.len());

                            header.type_mask |= TYPE_META;
                            if jpeg.is_some() {
                                header.type_mask |= TYPE_PIC;
                            }
                            header.meta_len = meta_section.len() as u32;
                            header.pic_len = jpeg_len as u32;

                            let mut inject = meta_section;
                            if let Some(j) = jpeg {
                                inject.extend_from_slice(&j);
                            }
                            pending_inject = Some(inject);
                            last_injected_title = Some(title.clone());
                            pass_remaining = pcm_bytes + header.pos_len as usize;
                            skip_remaining = orig_meta_len + orig_pic_len;

                            eprintln!(
                                "{} [GAPLESS] {} / {} / {} + {}b cover",
                                ts(),
                                title,
                                meta.artist,
                                meta.album,
                                jpeg_len
                            );
                        } else if has_meta && injected_this_start {
                            // STRIP: HQPlayer sent a META refresh — strip it
                            header.type_mask &= !TYPE_META;
                            header.type_mask &= !TYPE_PIC;
                            header.meta_len = 0;
                            header.pic_len = 0;
                            pending_inject = None;
                            pass_remaining = pcm_bytes + header.pos_len as usize;
                            skip_remaining = orig_meta_len + orig_pic_len;

                            if !strip_logged {
                                eprintln!(
                                    "{} [STRIP] META refresh stripped (frame {})",
                                    ts(),
                                    frame_count
                                );
                                strip_logged = true;
                            }
                        } else if injected_this_start
                            && !title.is_empty()
                            && frame_count % 300 == 0
                        {
                            // REFRESH: periodic re-injection (~30s)
                            let meta_section = build_meta_section(
                                &stream_params,
                                title,
                                &meta.artist,
                                &meta.album,
                            );

                            header.type_mask |= TYPE_META;
                            header.meta_len = meta_section.len() as u32;
                            // Don't touch pic_len — no cover art on refresh

                            pending_inject = Some(meta_section);
                            last_injected_title = Some(title.clone());
                            pass_remaining = pcm_bytes + header.pos_len as usize;
                            skip_remaining = orig_meta_len + orig_pic_len;

                            eprintln!(
                                "{} [REFRESH] {} (frame {})",
                                ts(),
                                title,
                                frame_count
                            );
                        } else {
                            // PASSTHROUGH: no metadata work needed
                            pending_inject = None;
                            pass_remaining = pcm_bytes
                                + header.pos_len as usize
                                + orig_meta_len
                                + orig_pic_len;
                            skip_remaining = 0;
                        }

                        out.extend_from_slice(&serialize_header(&header));
                        phase = Phase::Pass;
                    }
                }

                Phase::Pass => {
                    let available = data.len() - pos;
                    let take = std::cmp::min(available, pass_remaining);
                    out.extend_from_slice(&data[pos..pos + take]);
                    pos += take;
                    pass_remaining -= take;

                    if pass_remaining == 0 {
                        if let Some(inject) = pending_inject.take() {
                            out.extend_from_slice(&inject);
                        }
                        if skip_remaining > 0 {
                            phase = Phase::Skip;
                        } else {
                            phase = Phase::Header;
                        }
                    }
                }

                Phase::Skip => {
                    let available = data.len() - pos;
                    let take = std::cmp::min(available, skip_remaining);
                    pos += take; // discard bytes
                    skip_remaining -= take;

                    if skip_remaining == 0 {
                        phase = Phase::Header;
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
