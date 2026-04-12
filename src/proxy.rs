use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::TcpStream;

use crate::frame::{
    build_meta_section, is_corrupt, parse_header, parse_start_message, serialize_header,
    FrameHeader, StreamParams, FRAME_HEADER_SIZE, TYPE_META, TYPE_PIC, TYPE_POS,
};
use crate::metadata::{Metadata, SharedMetadata};
use crate::ts;

#[derive(Debug, PartialEq)]
pub(crate) enum FrameOp {
    /// Stream N bytes from src to dst.
    Pass(usize),
    /// Emit bytes to dst immediately (no src interaction).
    Emit(Vec<u8>),
    /// Discard N bytes from src.
    Skip(usize),
}

/// Drain ops against a data slice. Returns when either the queue is empty
/// or the head op can't be fully satisfied by the remaining source bytes
/// (in which case the head op is left with a reduced count).
pub(crate) fn execute_ops(
    ops: &mut VecDeque<FrameOp>,
    data: &[u8],
    pos: &mut usize,
    out: &mut Vec<u8>,
) {
    while let Some(op) = ops.front_mut() {
        let remaining = data.len().saturating_sub(*pos);
        match op {
            FrameOp::Pass(n) => {
                let take = (*n).min(remaining);
                out.extend_from_slice(&data[*pos..*pos + take]);
                *pos += take;
                *n -= take;
                if *n == 0 {
                    ops.pop_front();
                } else {
                    return;
                }
            }
            FrameOp::Skip(n) => {
                let take = (*n).min(remaining);
                *pos += take;
                *n -= take;
                if *n == 0 {
                    ops.pop_front();
                } else {
                    return;
                }
            }
            FrameOp::Emit(bytes) => {
                out.extend_from_slice(bytes);
                ops.pop_front();
            }
        }
    }
}

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
    pub(crate) ops: VecDeque<FrameOp>,
    pub(crate) header_buf: Vec<u8>,
    pub(crate) injected: bool,
    pub(crate) last_title: Option<String>,
    pub(crate) last_pos_state: Option<crate::metadata::PlayState>,
    pub(crate) frame_count: u64,
    pub(crate) strip_logged: bool,
    pub(crate) pending_meta_pic: Option<Vec<u8>>,
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
            ops: VecDeque::new(),
            header_buf: Vec::with_capacity(FRAME_HEADER_SIZE),
            injected: false,
            last_title: None,
            last_pos_state: None,
            frame_count: 0,
            strip_logged: false,
            pending_meta_pic: None,
        }
    }

    pub(crate) fn reset_for_start(&mut self, params: StreamParams) {
        self.params = params;
        self.injected = false;
        self.last_title = None;
        self.last_pos_state = None;
        self.frame_count = 0;
        self.strip_logged = false;
        self.header_buf.clear();
        self.ops.clear();
        self.pending_meta_pic = None;
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
        self.pending_meta_pic = Some(payload);
        self.last_title = Some(meta.title.clone());
        jpeg_len
    }

    pub(crate) fn strip(&mut self, header: &mut FrameHeader) {
        header.type_mask &= !(TYPE_META | TYPE_PIC);
        header.meta_len = 0;
        header.pic_len = 0;
        self.pending_meta_pic = None;
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
    ///
    /// HQPlayer's own NAA output emits POS every ~10 audio frames (verified
    /// 2026-04-12 via capture_proxy on einstein: mean gap 10.9 frames across
    /// 17 POS sections, ~2.33s wall-clock at 81920 samples/frame × 384kHz).
    /// Matching that cadence is the "mimic HQPlayer exactly" rule — T8 is
    /// tuned to that rate. Also inject immediately on first sight and on
    /// state transitions so the display catches up promptly.
    pub(crate) fn decide_pos_action(
        &self,
        pos: Option<&crate::metadata::PlaybackPosition>,
    ) -> PosAction {
        const POS_CADENCE_FRAMES: u64 = 10;

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

    /// Build the op sequence for one frame's body (everything after the header).
    /// Mutates `header` to reflect final meta_len/pic_len values.
    /// The header serialisation itself is pushed by the caller.
    pub(crate) fn build_frame_ops(&mut self, header: &mut FrameHeader) {
        use crate::frame::build_pos_section;

        let pcm_bytes = header.pcm_len as usize * self.params.bytes_per_sample as usize;
        let orig_pos_len = header.pos_len as usize;
        let orig_meta_len = header.meta_len as usize;
        let orig_pic_len = header.pic_len as usize;
        let has_meta = header.has_meta();

        let meta = self.shared.get();
        let title = meta.title.clone();
        let pos_ref = meta.position.as_ref();

        // --- Meta/pic decision (unchanged) ---
        let action = self.decide_action(has_meta, &title);
        let replace_meta_pic = match action {
            Action::Inject => {
                let jpeg_len = self.inject(header, &meta, true);
                self.injected = true;
                eprintln!(
                    "{} [INJECT] {} / {} / {} + {}b cover",
                    ts(), title, meta.artist, meta.album, jpeg_len,
                );
                true
            }
            Action::Gapless => {
                let jpeg_len = self.inject(header, &meta, true);
                eprintln!(
                    "{} [GAPLESS] {} / {} / {} + {}b cover",
                    ts(), title, meta.artist, meta.album, jpeg_len,
                );
                true
            }
            Action::Strip => {
                self.strip(header);
                if !self.strip_logged {
                    eprintln!(
                        "{} [STRIP] META stripped (frame {}, injected={})",
                        ts(), self.frame_count, self.injected,
                    );
                    self.strip_logged = true;
                }
                true
            }
            Action::Refresh => {
                self.inject(header, &meta, false);
                eprintln!(
                    "{} [REFRESH] {} (frame {})",
                    ts(), title, self.frame_count,
                );
                true
            }
            Action::Passthrough => false,
        };

        // --- POS decision ---
        let pos_action = self.decide_pos_action(pos_ref);
        let (pos_bytes, replace_pos): (Vec<u8>, bool) = match pos_action {
            PosAction::Inject => {
                let pos = pos_ref.expect("decide_pos_action returned Inject only when Some");
                let bytes = build_pos_section(pos, std::time::Instant::now());
                if self.last_pos_state.is_none() {
                    eprintln!(
                        "{} [POS] first inject: len={} state={:?} pos={} length={}",
                        ts(), bytes.len(), pos.state, pos.position_seconds, pos.length_seconds,
                    );
                }
                header.pos_len = bytes.len() as u32;
                header.type_mask |= TYPE_POS;
                self.last_pos_state = Some(pos.state);
                (bytes, true)
            }
            PosAction::Passthrough => (Vec::new(), false),
        };

        // --- Build op sequence ---
        // Body layout: [pcm][pos][meta][pic]
        if replace_pos {
            self.ops.push_back(FrameOp::Pass(pcm_bytes));
            if orig_pos_len > 0 {
                self.ops.push_back(FrameOp::Skip(orig_pos_len));
            }
            self.ops.push_back(FrameOp::Emit(pos_bytes));
        } else {
            // Coalesce PCM and pass-through POS into a single Pass.
            self.ops.push_back(FrameOp::Pass(pcm_bytes + orig_pos_len));
        }

        // Meta/pic region
        if replace_meta_pic {
            if orig_meta_len + orig_pic_len > 0 {
                self.ops.push_back(FrameOp::Skip(orig_meta_len + orig_pic_len));
            }
            if let Some(payload) = self.pending_meta_pic.take() {
                self.ops.push_back(FrameOp::Emit(payload));
            }
        } else {
            if orig_meta_len + orig_pic_len > 0 {
                self.ops.push_back(FrameOp::Pass(orig_meta_len + orig_pic_len));
            }
            self.pending_meta_pic = None;
        }
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
/// Processes NAA v6 binary frames, injecting Roon metadata and cover art
/// into the audio stream so the DAC displays track info. See
/// FrameProcessor::decide_action for the injection logic, and
/// FrameProcessor::build_frame_ops for how each frame is shaped.
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

        // Top-of-buffer XML check (before any frame processing)
        if proc.ops.is_empty() && proc.header_buf.is_empty() {
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

        let mut pos = 0;
        out.clear();

        while pos < data.len() {
            // If we have no pending ops, we're accumulating a header.
            if proc.ops.is_empty() {
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
                    break; // rest of buffer handled by XML path
                }

                // Accumulate header bytes (32 total)
                let need = FRAME_HEADER_SIZE - proc.header_buf.len();
                let take = need.min(data.len() - pos);
                proc.header_buf.extend_from_slice(&data[pos..pos + take]);
                pos += take;

                if proc.header_buf.len() < FRAME_HEADER_SIZE {
                    continue;
                }

                // Header complete — parse and build ops for this frame.
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

                proc.frame_count += 1;
                proc.build_frame_ops(&mut header);

                // Push the (possibly rewritten) header to the front so it emits first.
                let header_bytes = serialize_header(&header);
                proc.ops.push_front(FrameOp::Emit(header_bytes.to_vec()));
            }

            // Drain ops against the remaining data.
            execute_ops(&mut proc.ops, data, &mut pos, &mut out);
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
