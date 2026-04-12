use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use crate::frame::{
    build_meta_section, is_corrupt, parse_header, parse_start_message, serialize_header,
    FrameHeader, StreamParams, FRAME_HEADER_SIZE, TYPE_META, TYPE_PIC, TYPE_POS,
};
use crate::metadata::SharedMetadata;
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

/// How often we must re-emit the [metadata] section even when content
/// is unchanged. The T8 reverts the title to HQPlayer's "Roon" fallback
/// if META goes quiet for too long — cover and POS are held correctly.
pub(crate) const META_REFRESH_FRAMES: u64 = 300;

pub(crate) struct FrameProcessor {
    pub(crate) shared: SharedMetadata,
    pub(crate) params: StreamParams,
    pub(crate) ops: VecDeque<FrameOp>,
    pub(crate) header_buf: Vec<u8>,
    /// (title, artist, album) last emitted as META. Used for content-change
    /// detection (which also governs whether to bundle a fresh cover).
    pub(crate) last_meta_key: Option<(String, String, String)>,
    /// Frame number of the last META emission — drives the cadence refresh.
    pub(crate) last_meta_frame: u64,
    /// (length, seek rounded to integer seconds, state) last emitted as POS.
    pub(crate) last_pos_key: Option<(u32, u32, crate::metadata::PlayState)>,
    pub(crate) frame_count: u64,
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
            last_meta_key: None,
            last_meta_frame: 0,
            last_pos_key: None,
            frame_count: 0,
        }
    }

    pub(crate) fn reset_for_start(&mut self, params: StreamParams) {
        self.params = params;
        self.last_meta_key = None;
        self.last_meta_frame = 0;
        self.last_pos_key = None;
        self.frame_count = 0;
        self.header_buf.clear();
        self.ops.clear();
    }

    /// Build the op sequence for one frame's body (everything after the header).
    /// Mutates `header` to reflect final section lengths and type_mask bits.
    /// The header serialisation itself is pushed by the caller.
    ///
    /// Uniform rule for every section the proxy owns (POS, META, PIC):
    ///   1. Unconditionally strip HQPlayer's bytes from the outgoing header.
    ///   2. Emit a replacement only when Roon's value has changed since the
    ///      last one we sent. Between events the T8 holds the last section.
    pub(crate) fn build_frame_ops(&mut self, header: &mut FrameHeader) {
        use crate::frame::build_pos_section;

        let pcm_bytes = header.pcm_len as usize * self.params.bytes_per_sample as usize;
        let orig_pos_len = header.pos_len as usize;
        let orig_meta_len = header.meta_len as usize;
        let orig_pic_len = header.pic_len as usize;

        let meta = self.shared.get();

        // --- POS: strip HQP, emit ours on change ---
        let pos_key = match (meta.length_seconds, meta.seek_position, meta.play_state) {
            (Some(len), Some(seek), Some(state)) => Some((len, seek.max(0.0) as u32, state)),
            _ => None,
        };
        let pos_bytes: Option<Vec<u8>> = match pos_key {
            Some(key) if Some(key) != self.last_pos_key => {
                let (len, seek_int, state) = key;
                let bytes = build_pos_section(
                    len,
                    meta.seek_position.unwrap(),
                    state,
                    1,
                    meta.tracks_total.max(1),
                );
                eprintln!(
                    "{} [POS] emit len={} seek={} state={:?}",
                    ts(), len, seek_int, state,
                );
                self.last_pos_key = Some(key);
                Some(bytes)
            }
            _ => None,
        };

        // --- META/PIC: strip HQP, emit ours on change or cadence refresh ---
        // Two triggers:
        //   1. content_changed — (title, artist, album) differs from last
        //      emission. Bundle a fresh cover.
        //   2. cadence refresh — text-only re-emit every META_REFRESH_FRAMES
        //      frames so the T8 doesn't revert the title.
        let meta_key: Option<(String, String, String)> = if meta.title.is_empty() {
            None
        } else {
            Some((meta.title.clone(), meta.artist.clone(), meta.album.clone()))
        };
        let content_changed = meta_key.is_some() && self.last_meta_key != meta_key;
        let refresh_due = meta_key.is_some()
            && self
                .frame_count
                .saturating_sub(self.last_meta_frame)
                >= META_REFRESH_FRAMES;
        // Emitted META section bytes, and — separately — whether to emit a
        // fresh PIC section with the cover. PIC only rides along on content
        // changes; cadence refreshes are text-only so we don't resend the
        // full JPEG every ~70s.
        let meta_section: Option<Vec<u8>> =
            if meta_key.is_some() && (content_changed || refresh_due) {
                Some(build_meta_section(
                    &self.params,
                    &meta.title,
                    &meta.artist,
                    &meta.album,
                ))
            } else {
                None
            };
        let cover_bytes: Option<Arc<Vec<u8>>> = if content_changed {
            meta.cover_art.clone()
        } else {
            None
        };
        if meta_section.is_some() {
            let kind = if content_changed { "change" } else { "refresh" };
            eprintln!(
                "{} [META] {} {} / {} / {} (frame {})",
                ts(), kind, meta.title, meta.artist, meta.album, self.frame_count,
            );
            self.last_meta_key = meta_key;
            self.last_meta_frame = self.frame_count;
        }

        // --- Rewrite header ---
        // Always strip every section the proxy owns; set bits/lengths back
        // only for the sections we're actually emitting this frame.
        header.type_mask &= !(TYPE_POS | TYPE_META | TYPE_PIC);
        header.pos_len = 0;
        header.meta_len = 0;
        header.pic_len = 0;
        if let Some(ref b) = pos_bytes {
            header.type_mask |= TYPE_POS;
            header.pos_len = b.len() as u32;
        }
        if let Some(ref b) = meta_section {
            header.type_mask |= TYPE_META;
            header.meta_len = b.len() as u32;
        }
        if let Some(ref jpeg) = cover_bytes {
            header.type_mask |= TYPE_PIC;
            header.pic_len = jpeg.len() as u32;
        }

        // --- Build op sequence ---
        // Body layout: [pcm][pos][meta][pic]
        self.ops.push_back(FrameOp::Pass(pcm_bytes));
        if orig_pos_len > 0 {
            self.ops.push_back(FrameOp::Skip(orig_pos_len));
        }
        if let Some(b) = pos_bytes {
            self.ops.push_back(FrameOp::Emit(b));
        }
        if orig_meta_len + orig_pic_len > 0 {
            self.ops.push_back(FrameOp::Skip(orig_meta_len + orig_pic_len));
        }
        if let Some(b) = meta_section {
            self.ops.push_back(FrameOp::Emit(b));
        }
        if let Some(jpeg) = cover_bytes {
            self.ops.push_back(FrameOp::Emit((*jpeg).clone()));
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
