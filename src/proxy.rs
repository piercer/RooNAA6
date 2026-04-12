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
pub(crate) enum Phase {
    Header,
    Pass,
    Skip,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Action {
    Inject,
    Gapless,
    Strip,
    Refresh,
    Passthrough,
}

#[derive(Debug, PartialEq)]
pub(crate) enum PosAction {
    Inject,
    Passthrough,
}

pub(crate) struct FrameProcessor {
    pub(crate) shared: SharedMetadata,
    pub(crate) params: StreamParams,
    pub(crate) phase: Phase,
    pub(crate) pass_remaining: usize,
    pub(crate) skip_remaining: usize,
    pub(crate) pending_inject: Option<Vec<u8>>,
    pub(crate) header_buf: Vec<u8>,
    pub(crate) injected: bool,
    pub(crate) last_title: Option<String>,
    pub(crate) last_pos_state: Option<crate::metadata::PlayState>,
    pub(crate) frame_count: u64,
    pub(crate) strip_logged: bool,
}

impl FrameProcessor {
    pub(crate) fn new(shared: SharedMetadata) -> Self {
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
            last_pos_state: None,
            frame_count: 0,
            strip_logged: false,
        }
    }

    pub(crate) fn reset_for_start(&mut self, params: StreamParams) {
        self.params = params;
        self.phase = Phase::Header;
        self.injected = false;
        self.last_title = None;
        self.last_pos_state = None;
        self.frame_count = 0;
        self.strip_logged = false;
        self.header_buf.clear();
        self.pass_remaining = 0;
        self.skip_remaining = 0;
        self.pending_inject = None;
    }

    /// Build and set the injection payload. Returns JPEG size for logging.
    pub(crate) fn inject(&mut self, header: &mut FrameHeader, meta: &Metadata, with_cover: bool) -> usize {
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

    pub(crate) fn strip(&mut self, header: &mut FrameHeader) {
        header.type_mask &= !(TYPE_META | TYPE_PIC);
        header.meta_len = 0;
        header.pic_len = 0;
        self.pending_inject = None;
    }

    /// Pure decision function: given frame has_meta bit and current Roon title,
    /// decide what to do with this frame. No side effects.
    pub(crate) fn decide_action(&self, has_meta: bool, title: &str) -> Action {
        if !title.is_empty() && !self.injected {
            // First injection — trigger on any frame once Roon title is known.
            // We don't wait for HQPlayer's META frame because it may have
            // already been stripped (see next branch).
            Action::Inject
        } else if has_meta && !self.injected {
            // HQPlayer sent META before Roon metadata arrived.
            // Strip it so T8 doesn't show HQP's fallback title ("Roon").
            Action::Strip
        } else if !title.is_empty()
            && self.injected
            && self.last_title.as_deref() != Some(title)
        {
            Action::Gapless
        } else if has_meta && self.injected {
            Action::Strip
        } else if self.injected && !title.is_empty() && self.frame_count % 300 == 0 {
            Action::Refresh
        } else {
            Action::Passthrough
        }
    }

    /// Pure decision: given current playback position, decide whether to inject
    /// a new [position] section this frame.
    pub(crate) fn decide_pos_action(
        &self,
        pos: Option<&crate::metadata::PlaybackPosition>,
    ) -> PosAction {
        const POS_CADENCE_FRAMES: u64 = 20;

        let Some(pos) = pos else {
            return PosAction::Passthrough;
        };

        if self.last_pos_state.is_none() {
            return PosAction::Inject;
        }

        if self.last_pos_state != Some(pos.state) {
            return PosAction::Inject;
        }

        if self.frame_count % POS_CADENCE_FRAMES == 0 {
            return PosAction::Inject;
        }

        PosAction::Passthrough
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
/// Actions (see FrameProcessor::decide_action):
/// - INJECT: first frame once Roon title is known -- inject title/artist/album + cover
/// - GAPLESS: track change during gapless playback -- inject new metadata + cover
/// - STRIP: either HQPlayer META before Roon title (avoid "Roon" leaking to T8),
///   or HQPlayer META refresh after we've already injected
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
                    let action = proc.decide_action(has_meta, title);
                    let replace_original = match action {
                        Action::Inject => {
                            let jpeg_len = proc.inject(&mut header, &meta, true);
                            proc.injected = true;
                            eprintln!(
                                "{} [INJECT] {} / {} / {} + {}b cover",
                                ts(), title, meta.artist, meta.album, jpeg_len,
                            );
                            true
                        }
                        Action::Gapless => {
                            let jpeg_len = proc.inject(&mut header, &meta, true);
                            eprintln!(
                                "{} [GAPLESS] {} / {} / {} + {}b cover",
                                ts(), title, meta.artist, meta.album, jpeg_len,
                            );
                            true
                        }
                        Action::Strip => {
                            proc.strip(&mut header);
                            if !proc.strip_logged {
                                eprintln!(
                                    "{} [STRIP] META stripped (frame {}, injected={})",
                                    ts(), proc.frame_count, proc.injected,
                                );
                                proc.strip_logged = true;
                            }
                            true
                        }
                        Action::Refresh => {
                            proc.inject(&mut header, &meta, false);
                            eprintln!(
                                "{} [REFRESH] {} (frame {})",
                                ts(), title, proc.frame_count,
                            );
                            true
                        }
                        Action::Passthrough => {
                            proc.pending_inject = None;
                            false
                        }
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
