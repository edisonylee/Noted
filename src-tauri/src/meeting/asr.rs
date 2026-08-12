// Live transcription: drain capture buffers → energy-gate VAD chunking →
// whisper on closed segments → DB insert + "meeting-segment" event.
//
// The chunker is deterministic Rust (no model) so it's unit-testable and can't
// hard-fail: frames of 30 ms; a segment opens after `open_frames` consecutive
// voiced frames (with pre-roll so onsets aren't clipped) and closes after
// `hang_ms` of quiet or at `max_segment_ms`. Silence is never sent to whisper —
// which also sidesteps whisper's hallucinate-on-silence failure mode.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc::SyncSender, Arc};
use std::time::Duration;

use anyhow::{anyhow, Result};
use serde_json::json;
use tauri::{Emitter, Manager};

use super::capture::ChannelBuf;
use super::store;
use crate::db::Db;

pub const SR: usize = 16_000;
const FRAME: usize = 480; // 30 ms @ 16 kHz
const FRAME_MS: u64 = 30;
const PREROLL_FRAMES: usize = 5; // 150 ms kept before a detected onset

#[derive(Clone, Copy)]
pub struct ChunkerCfg {
    pub open_rms: f32,
    pub close_rms: f32,
    pub open_frames: u32,
    pub hang_ms: u64,
    pub max_segment_ms: u64,
    pub min_segment_ms: u64,
}

impl Default for ChunkerCfg {
    fn default() -> Self {
        Self {
            open_rms: 0.012,
            close_rms: 0.006,
            open_frames: 3,
            hang_ms: 720,
            // Shorter transcript rows materially improve attribution: a
            // 25-second energy-gated chunk often spans two standup turns, but
            // the transcript schema can carry only one speaker per row.
            max_segment_ms: 12_000,
            min_segment_ms: 400,
        }
    }
}

pub struct PendingSegment {
    pub t0_ms: i64,
    pub t1_ms: i64,
    /// Frames above the closing speech threshold. Unlike the enclosing span,
    /// this excludes pre-roll and the quiet hang used to keep words intact.
    pub voiced_ms: i64,
    pub samples: Vec<f32>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RetainedSegment {
    pub t0_ms: i64,
    pub t1_ms: i64,
    #[serde(default)]
    pub voiced_ms: Option<i64>,
    pub text: String,
}

fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()
}

/// Energy-gate segmenter over a continuous mono 16 kHz stream.
pub struct Chunker {
    cfg: ChunkerCfg,
    carry: Vec<f32>,
    frame_idx: u64, // absolute frames consumed since stream start
    preroll: Vec<Vec<f32>>,
    voiced_run: u32,
    in_speech: bool,
    seg: Vec<f32>,
    seg_start_frame: u64,
    quiet_ms: u64,
}

impl Chunker {
    pub fn new(cfg: ChunkerCfg) -> Self {
        Self {
            cfg,
            carry: Vec::new(),
            frame_idx: 0,
            preroll: Vec::new(),
            voiced_run: 0,
            in_speech: false,
            seg: Vec::new(),
            seg_start_frame: 0,
            quiet_ms: 0,
        }
    }

    /// Feed samples; returns any segments that closed.
    pub fn push(&mut self, samples: &[f32]) -> Vec<PendingSegment> {
        let mut out = Vec::new();
        self.carry.extend_from_slice(samples);
        let mut off = 0;
        while self.carry.len() - off >= FRAME {
            let frame: Vec<f32> = self.carry[off..off + FRAME].to_vec();
            off += FRAME;
            self.step(&frame, &mut out);
        }
        self.carry.drain(..off);
        out
    }

    fn step(&mut self, frame: &[f32], out: &mut Vec<PendingSegment>) {
        let level = rms(frame);
        self.frame_idx += 1;

        if !self.in_speech {
            self.preroll.push(frame.to_vec());
            if self.preroll.len() > PREROLL_FRAMES {
                self.preroll.remove(0);
            }
            if level >= self.cfg.open_rms {
                self.voiced_run += 1;
            } else {
                self.voiced_run = 0;
            }
            if self.voiced_run >= self.cfg.open_frames {
                // Open: segment starts where the preroll begins.
                self.in_speech = true;
                self.quiet_ms = 0;
                self.seg_start_frame = self.frame_idx - self.preroll.len() as u64;
                self.seg = self.preroll.concat();
                self.preroll.clear();
                self.voiced_run = 0;
            }
            return;
        }

        self.seg.extend_from_slice(frame);
        if level < self.cfg.close_rms {
            self.quiet_ms += FRAME_MS;
        } else {
            self.quiet_ms = 0;
        }
        let seg_ms = (self.seg.len() / (SR / 1000)) as u64;
        if self.quiet_ms >= self.cfg.hang_ms || seg_ms >= self.cfg.max_segment_ms {
            self.close(out);
        }
    }

    fn close(&mut self, out: &mut Vec<PendingSegment>) {
        let seg_ms = (self.seg.len() / (SR / 1000)) as u64;
        if seg_ms >= self.cfg.min_segment_ms {
            let t0_ms = (self.seg_start_frame * FRAME_MS) as i64;
            let voiced_ms = self
                .seg
                .chunks(FRAME)
                .filter(|frame| rms(frame) >= self.cfg.close_rms)
                .count() as i64
                * FRAME_MS as i64;
            out.push(PendingSegment {
                t0_ms,
                t1_ms: t0_ms + seg_ms as i64,
                voiced_ms,
                samples: std::mem::take(&mut self.seg),
            });
        } else {
            self.seg.clear();
        }
        self.in_speech = false;
        self.quiet_ms = 0;
    }

    /// End of stream: close any open segment.
    pub fn flush(&mut self) -> Option<PendingSegment> {
        if !self.in_speech {
            return None;
        }
        let mut out = Vec::new();
        self.close(&mut out);
        out.pop()
    }

    /// Stream position (where the next pushed sample lands), in ms.
    pub fn pos_ms(&self) -> u64 {
        self.frame_idx * FRAME_MS
    }

    /// The source stopped delivering for `gap_ms` (the system tap goes quiet
    /// until an app plays audio; session rebuilds drop samples): close any
    /// open segment and jump the timeline forward so both channels stay
    /// anchored to the meeting's wall clock — otherwise their transcripts
    /// interleave in the wrong order.
    pub fn advance_gap(&mut self, gap_ms: u64, out: &mut Vec<PendingSegment>) {
        if self.in_speech {
            self.close(out);
        }
        self.carry.clear();
        self.preroll.clear();
        self.voiced_run = 0;
        self.frame_idx += gap_ms / FRAME_MS;
    }
}

// ---------------------------------------------------------------------------
// ASR engines. Whisper (whisper.cpp) is the default; Parakeet (NVIDIA
// Parakeet-TDT 0.6B, a NeMo transducer running on the sherpa-onnx runtime we
// already link for speaker embeddings) is faster and stronger on proper
// nouns. Both consume the same VAD-chunked 16 kHz mono segments.
// ---------------------------------------------------------------------------

/// Which engine to load, resolved from config + downloaded files at meeting
/// start (`meeting::engine_spec`).
pub enum EngineSpec {
    Whisper {
        model: PathBuf,
    },
    /// Directory holding encoder/decoder/joiner int8 ONNX + tokens.txt.
    Parakeet {
        dir: PathBuf,
    },
    Hosted {
        vocabulary: Vec<String>,
    },
    Byok {
        vocabulary: Vec<String>,
        provider: String,
        model: String,
    },
}

impl EngineSpec {
    /// Stable, non-secret provenance saved with the meeting before capture
    /// begins. This records the resolved engine, including local fallbacks.
    pub fn provenance(&self) -> (String, String) {
        match self {
            Self::Whisper { model } => (
                "whisper".into(),
                model
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("whisper-model")
                    .to_string(),
            ),
            Self::Parakeet { dir } => (
                "parakeet".into(),
                dir.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("parakeet-tdt-0.6b")
                    .to_string(),
            ),
            Self::Hosted { .. } => ("hosted".into(), "parakeet-tdt-0.6b".into()),
            Self::Byok {
                provider, model, ..
            } => (provider.clone(), model.clone()),
        }
    }
}

/// Model stays loaded for the whole meeting. Calls are serialized by the
/// worker loop so Metal/onnxruntime never see two decodes at once.
pub enum Transcriber {
    Whisper {
        ctx: whisper_rs::WhisperContext,
        /// Decoder bias for domain terms and names (whisper's initial_prompt).
        hint: Option<String>,
    },
    /// No prompt biasing: the published Parakeet export ships no bpe.vocab,
    /// so sherpa hotwords can't be used — `apply_vocab` still canonicalizes
    /// vocabulary terms after decode.
    Parakeet {
        rec: sherpa_rs::transducer::TransducerRecognizer,
    },
    Hosted {
        session: crate::hosted::Session,
    },
    Byok {
        vocabulary: Vec<String>,
    },
}

impl Transcriber {
    pub fn new(spec: &EngineSpec, hint: Option<String>) -> Result<Self> {
        match spec {
            EngineSpec::Whisper { model } => {
                let path = model.to_str().ok_or_else(|| anyhow!("bad model path"))?;
                let ctx = whisper_rs::WhisperContext::new_with_params(
                    path,
                    whisper_rs::WhisperContextParameters::default(),
                )
                .map_err(|e| anyhow!("whisper load failed: {e:?}"))?;
                // set_initial_prompt panics on interior NULs.
                let hint = hint.map(|h| h.replace('\0', ""));
                Ok(Self::Whisper { ctx, hint })
            }
            EngineSpec::Parakeet { dir } => {
                let p = |f: &str| dir.join(f).to_string_lossy().to_string();
                let rec = sherpa_rs::transducer::TransducerRecognizer::new(
                    sherpa_rs::transducer::TransducerConfig {
                        encoder: p("encoder.int8.onnx"),
                        decoder: p("decoder.int8.onnx"),
                        joiner: p("joiner.int8.onnx"),
                        tokens: p("tokens.txt"),
                        model_type: "nemo_transducer".into(),
                        decoding_method: "greedy_search".into(),
                        sample_rate: SR as i32,
                        feature_dim: 80,
                        num_threads: 4,
                        ..Default::default()
                    },
                )
                .map_err(|e| anyhow!("parakeet load failed: {e}"))?;
                Ok(Self::Parakeet { rec })
            }
            EngineSpec::Hosted { vocabulary } => Ok(Self::Hosted {
                session: crate::hosted::Session::open(vocabulary.clone())?,
            }),
            EngineSpec::Byok { vocabulary, .. } => Ok(Self::Byok {
                vocabulary: vocabulary.clone(),
            }),
        }
    }

    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        match self {
            Self::Whisper { ctx, hint } => {
                let mut state = ctx
                    .create_state()
                    .map_err(|e| anyhow!("whisper state: {e:?}"))?;
                let mut params =
                    whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy {
                        best_of: 1,
                    });
                params.set_language(Some("en"));
                params.set_print_progress(false);
                params.set_print_special(false);
                params.set_print_realtime(false);
                params.set_print_timestamps(false);
                if let Some(h) = hint {
                    // whisper-rs leaks a small CString per call here — bounded
                    // at ~1 KB per segment, dwarfed by decode-state churn.
                    params.set_initial_prompt(h);
                }
                state
                    .full(params, samples)
                    .map_err(|e| anyhow!("transcribe failed: {e:?}"))?;
                let mut out = String::new();
                for i in 0..state.full_n_segments() {
                    if let Some(seg) = state.get_segment(i) {
                        if let Ok(t) = seg.to_str() {
                            out.push_str(t);
                        }
                    }
                }
                Ok(out.trim().to_string())
            }
            Self::Parakeet { rec } => Ok(rec.transcribe(SR as u32, samples).trim().to_string()),
            Self::Hosted { session } => session.transcribe(samples),
            Self::Byok { vocabulary } => {
                crate::provider::byok_transcribe_blocking(samples, vocabulary)
            }
        }
    }

    pub fn finish(&self) {
        if let Self::Hosted { session } = self {
            session.finalize();
        }
    }
}

/// Whisper emits bracketed sound tags on non-speech ("[BLANK_AUDIO]", "(music)",
/// "*ding*", "*sad music*") and lone punctuation ("."). A segment that is only
/// such tokens carries no words.
pub fn is_junk(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    // Whole-line sound tag, possibly multi-word ("*sad music*", "(door slams)").
    for (open, close) in [('*', '*'), ('(', ')'), ('[', ']')] {
        if t.starts_with(open) && t.ends_with(close) && t.len() >= 2 {
            return true;
        }
    }
    t.split_whitespace().all(|w| {
        (w.starts_with('[') && w.ends_with(']'))
            || (w.starts_with('(') && w.ends_with(')'))
            || (w.starts_with('*') && w.ends_with('*'))
            || !w.chars().any(|c| c.is_alphanumeric())
    })
}

fn decode_key(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .flat_map(|c| c.to_lowercase())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Preserve the first short utterance but reject an identical immediate repeat.
/// This is a last-line defense against decoder loops such as silence repeatedly
/// becoming "Thank you"; real longer repeated statements remain untouched.
fn repeated_short_decode(last: Option<(&str, i64)>, text: &str, t0_ms: i64) -> bool {
    let key = decode_key(text);
    key.split_whitespace().count() <= 4
        && last.is_some_and(|(previous, previous_t1)| {
            t0_ms - previous_t1 <= 30_000 && key == decode_key(previous)
        })
}

/// Samples quieter than this don't vote in the zero-crossing count (below the
/// chunker's close threshold — gate hang and near-silence must not dilute it).
const ZC_FLOOR: f32 = 0.004;
/// Speech crosses zero hundreds of times a second — even a low male hum at
/// 85 Hz manages ~170/s, and harmonics/fricatives push real speech far higher.
/// Mains hum, rumble, and format-mangled audio (a stereo/48 kHz stream misread
/// as mono/16 kHz) sit well below.
const ZC_SPEECH_PER_SEC: f32 = 100.0;

/// Deterministic non-speech gate on a closed segment. The energy gate passes
/// anything loud enough, but whisper hallucinates fluent text ("Thank you.")
/// over low-frequency junk; energy can't tell hum from voice — zero-crossing
/// density can. Rate is measured over the significant samples only, so a
/// segment that is mostly gate hang still judges its voiced core.
pub fn looks_like_speech(samples: &[f32]) -> bool {
    let mut significant = 0u64;
    let mut crossings = 0u64;
    let mut last_sign = 0i8;
    for &s in samples {
        if s.abs() < ZC_FLOOR {
            continue;
        }
        significant += 1;
        let sign = if s > 0.0 { 1 } else { -1 };
        if last_sign != 0 && sign != last_sign {
            crossings += 1;
        }
        last_sign = sign;
    }
    if significant == 0 {
        return false;
    }
    crossings as f32 * SR as f32 / significant as f32 >= ZC_SPEECH_PER_SEC
}

/// Re-run the meeting VAD + ASR pipeline over a retained 16 kHz mono WAV.
/// This is intentionally DB-free: recovery can be inspected and backed up as
/// JSON before a caller chooses to replace any persisted transcript rows.
pub fn transcribe_retained_wav(
    engine: &EngineSpec,
    wav_path: &Path,
    hint: Option<String>,
    vocab: &[String],
) -> Result<Vec<RetainedSegment>> {
    let mut reader = hound::WavReader::open(wav_path)?;
    let spec = reader.spec();
    if spec.channels != 1
        || spec.sample_rate != SR as u32
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err(anyhow!("retained meeting WAV must be 16 kHz mono i16"));
    }
    let mut transcriber = Transcriber::new(engine, hint)?;
    let mut chunker = Chunker::new(ChunkerCfg::default());
    let mut out = Vec::new();
    let mut pcm = Vec::with_capacity(SR);

    let decode = |segments: Vec<PendingSegment>,
                  transcriber: &mut Transcriber,
                  out: &mut Vec<RetainedSegment>|
     -> Result<()> {
        for seg in segments {
            if !looks_like_speech(&seg.samples) {
                continue;
            }
            let text = apply_vocab(&transcriber.transcribe(&seg.samples)?, vocab);
            if is_junk(&text) {
                continue;
            }
            if repeated_short_decode(
                out.last().map(|s| (s.text.as_str(), s.t1_ms)),
                &text,
                seg.t0_ms,
            ) {
                continue;
            }
            out.push(RetainedSegment {
                t0_ms: seg.t0_ms,
                t1_ms: seg.t1_ms,
                voiced_ms: Some(seg.voiced_ms),
                text,
            });
        }
        Ok(())
    };

    for sample in reader.samples::<i16>() {
        pcm.push(sample? as f32 / 32768.0);
        if pcm.len() == SR {
            decode(chunker.push(&pcm), &mut transcriber, &mut out)?;
            pcm.clear();
        }
    }
    if !pcm.is_empty() {
        decode(chunker.push(&pcm), &mut transcriber, &mut out)?;
    }
    decode(
        chunker.flush().into_iter().collect(),
        &mut transcriber,
        &mut out,
    )?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Vocabulary: whisper knows English, not this user's world — "a16z" comes out
// "a sixteen z" and colleague names get respelled. Two defenses: an
// initial-prompt decoder bias (names + user vocabulary), and a deterministic
// canonicalizer that rewrites near-miss spellings after decode.
// ---------------------------------------------------------------------------

/// Whisper keeps only the tail of an over-long initial prompt, so the
/// user-curated vocabulary goes last (names are re-learnable from context;
/// jargon isn't).
const HINT_CAP_CHARS: usize = 700;

/// Build whisper's initial-prompt bias from people names + user vocabulary.
/// Returns None when there is nothing to bias toward.
pub fn vocab_hint(names: &[String], vocab: &[String]) -> Option<String> {
    let clean = |list: &[String]| -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        list.iter()
            .map(|s| s.trim().replace(['\0', '\n'], " "))
            .filter(|s| !s.is_empty())
            .filter(|s| seen.insert(s.to_lowercase()))
            .collect()
    };
    let names = clean(names);
    let vocab = clean(vocab);
    let mut parts = Vec::new();
    if !names.is_empty() {
        parts.push(format!("Participants: {}.", names.join(", ")));
    }
    if !vocab.is_empty() {
        parts.push(format!("Vocabulary: {}.", vocab.join(", ")));
    }
    if parts.is_empty() {
        return None;
    }
    let mut hint = parts.join(" ");
    if hint.len() > HINT_CAP_CHARS {
        // Trim from the front so the vocabulary tail survives, on a char
        // boundary, from the start of a term.
        let cut = hint.len() - HINT_CAP_CHARS;
        let cut = hint[cut..].find(' ').map(|i| cut + i + 1).unwrap_or(cut);
        hint = hint[cut..].to_string();
    }
    Some(hint)
}

/// Word tokens (consecutive alphanumerics) with byte ranges.
fn word_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            spans.push((s, i));
        }
    }
    if let Some(s) = start {
        spans.push((s, text.len()));
    }
    spans
}

/// Canonicalize vocabulary terms in decoded text: a run of whole words whose
/// alphanumeric collapse equals a term's ("a 16 z", "A16-Z", "a16z" for term
/// "a16z") is rewritten to the canonical spelling. Deterministic; runs never
/// start or end inside a word, and multi-word runs must collapse to ≥ 3 chars
/// so "a i" can't become "AI".
pub fn apply_vocab(text: &str, vocab: &[String]) -> String {
    let collapse = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect()
    };
    let terms: Vec<(&String, String)> = vocab
        .iter()
        .map(|t| (t, collapse(t)))
        .filter(|(_, c)| c.len() >= 2)
        .collect();
    if terms.is_empty() {
        return text.to_string();
    }
    let spans = word_spans(text);
    let words: Vec<String> = spans.iter().map(|&(a, b)| collapse(&text[a..b])).collect();

    let mut out = String::with_capacity(text.len());
    let mut consumed = 0; // bytes of `text` already emitted
    let mut i = 0;
    while i < spans.len() {
        let mut matched: Option<(usize, &String)> = None; // (last word idx, canonical)
        for (canon, target) in &terms {
            let mut acc = String::new();
            for j in i..spans.len() {
                // Words in a run may only be separated by light punctuation
                // ("a 16 z", "A.16.Z"), never across sentence-sized gaps —
                // a period followed by whitespace is a sentence boundary.
                if j > i {
                    let gap = &text[spans[j - 1].1..spans[j].0];
                    if gap.len() > 2
                        || !gap.chars().all(|c| " -.'’".contains(c))
                        || (gap.contains('.') && gap.contains(' '))
                    {
                        break;
                    }
                }
                acc.push_str(&words[j]);
                if acc.len() >= target.len() {
                    if acc == *target && (j == i || target.len() >= 3) {
                        matched = Some((j, canon));
                    }
                    break;
                }
            }
            if matched.is_some() {
                break;
            }
        }
        match matched {
            Some((j, canon)) => {
                out.push_str(&text[consumed..spans[i].0]);
                out.push_str(canon);
                consumed = spans[j].1;
                i = j + 1;
            }
            None => i += 1,
        }
    }
    out.push_str(&text[consumed..]);
    out
}

// ---------------------------------------------------------------------------
// Echo suppression: without headphones the mic hears the speakers, so remote
// speech shows up twice — once clean on "them", once degraded on "me".
// ---------------------------------------------------------------------------

/// Does `mic_text` look like the mic's rendition of `system_text` spoken at
/// the same time? Distinct-token containment: whisper mangles echo audio
/// (drops words, hallucinates fillers), so we ask what fraction of the mic's
/// words the system channel also heard. Short utterances ("yeah", "thank
/// you") are never suppressed — both sides genuinely say them.
pub fn is_echo(mic_text: &str, system_text: &str) -> bool {
    let tokens = |s: &str| -> std::collections::HashSet<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(|w| w.to_lowercase())
            .collect()
    };
    let mic = tokens(mic_text);
    if mic.len() < 4 {
        return false;
    }
    let sys = tokens(system_text);
    let hits = mic.iter().filter(|t| sys.contains(*t)).count();
    hits as f32 / mic.len() as f32 >= 0.70
}

/// How far apart matching segments may sit across channels. The microphone
/// hears speaker output at the same wall time, but its quieter copy can open
/// and close on different VAD boundaries.
const ECHO_SLACK_MS: i64 = 2_500;

struct RecentSeg {
    id: i64,
    t0: i64,
    t1: i64,
    text: String,
}

/// Concatenated text of recent segments overlapping [t0, t1] +/- slack.
fn overlapping_text(recents: &[RecentSeg], t0: i64, t1: i64) -> String {
    recents
        .iter()
        .filter(|r| r.t1 >= t0 - ECHO_SLACK_MS && r.t0 <= t1 + ECHO_SLACK_MS)
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Worker: the per-meeting loop tying capture → chunker → whisper → DB/UI.
// ---------------------------------------------------------------------------

struct ChannelPipe {
    name: &'static str,
    buf: Arc<ChannelBuf>,
    chunker: Chunker,
    wav: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>,
    last_decode: Option<(String, i64)>,
}

type RetainedWav = hound::WavWriter<std::io::BufWriter<std::fs::File>>;

fn wav_writer(path: &Path) -> Result<RetainedWav> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    Ok(hound::WavWriter::create(path, spec)?)
}

fn retained_writers(dir: &Path) -> Result<(RetainedWav, RetainedWav)> {
    std::fs::create_dir_all(dir)?;
    Ok((
        wav_writer(&dir.join("me.wav"))?,
        wav_writer(&dir.join("them.wav"))?,
    ))
}

pub struct WorkerArgs {
    pub meeting_id: i64,
    pub me: Arc<ChannelBuf>,
    pub them: Arc<ChannelBuf>,
    pub stop: Arc<AtomicBool>,
    pub engine: EngineSpec,
    /// Meeting start waits for this signal before capture threads or UI state
    /// are committed. An ASR/session failure is therefore a visible start
    /// error, never a fake recording that produces no transcript.
    pub ready: SyncSender<std::result::Result<(), String>>,
    /// None = audio retention off. Some(dir) = write me.wav / them.wav there.
    pub audio_dir: Option<PathBuf>,
    /// Wall anchor for both channel timelines (gap insertion keys off it).
    pub started_epoch_ms: u64,
    /// Speaker-embedding model, when downloaded — None = no diarization.
    pub speaker_model: Option<PathBuf>,
    /// Whisper initial-prompt bias (names + vocabulary), when there is one.
    pub asr_hint: Option<String>,
    /// Canonical user-vocabulary terms for post-decode normalization.
    pub vocab: Vec<String>,
    /// Calendar attendees are diagnostic context only. They never become
    /// speaker labels without identity evidence.
    pub attendees: Vec<String>,
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Wall time may run ahead of a channel's sample count by this much before we
/// declare a delivery gap (covers drain-tick + device latency jitter).
const GAP_SLACK_MS: u64 = 2_500;
/// Gap zeros written into the retained WAV are capped so a laptop asleep for
/// hours can't bloat the file; the timeline itself always advances fully.
const WAV_GAP_CAP_MS: u64 = 30 * 60 * 1_000;

pub fn run_worker(app: tauri::AppHandle, args: WorkerArgs) {
    let ready = args.ready;
    let mut transcriber = match Transcriber::new(&args.engine, args.asr_hint.clone()) {
        Ok(t) => t,
        Err(e) => {
            let message = format!("meeting transcription could not start: {e}");
            eprintln!("[noted] {message}");
            let _ = ready.send(Err(message));
            return;
        }
    };

    let (me_wav, them_wav) = match &args.audio_dir {
        Some(dir) => match retained_writers(dir) {
            Ok((me, them)) => (Some(me), Some(them)),
            Err(e) => {
                let message = format!("meeting audio files could not be created: {e}");
                eprintln!("[noted] {message}");
                let _ = ready.send(Err(message));
                return;
            }
        },
        None => (None, None),
    };

    // Speaker-model startup can take several seconds on a cold launch.  Do it
    // before declaring the worker ready; otherwise capture begins and the UI
    // timer runs while this thread is still unable to drain or transcribe the
    // mic.  Audio was buffered, but the apparent dead period made a healthy
    // microphone look broken.
    let mut embedder = args.speaker_model.as_ref().and_then(|p| {
        super::diarize::Embedder::new(p)
            .map_err(|e| eprintln!("[noted] speaker embeddings unavailable: {e}"))
            .ok()
    });

    if ready.send(Ok(())).is_err() {
        return;
    }

    let mut voice_prints: Vec<super::diarize::SegEmb> = Vec::new();

    let mut prints_since_relabel = 0usize;

    // "them" first: it's the clean digital copy, so its segments are already
    // recorded by the time the mic's echo rendition of the same speech closes.
    let mut pipes = [
        ChannelPipe {
            name: "them",
            buf: args.them.clone(),
            chunker: Chunker::new(ChunkerCfg::default()),
            wav: them_wav,
            last_decode: None,
        },
        ChannelPipe {
            name: "me",
            buf: args.me.clone(),
            chunker: Chunker::new(ChunkerCfg::default()),
            wav: me_wav,
            last_decode: None,
        },
    ];
    let mut recent_them: Vec<RecentSeg> = Vec::new();
    let mut recent_me: Vec<RecentSeg> = Vec::new();

    loop {
        let stopping = args.stop.load(Ordering::Relaxed);
        for pipe in pipes.iter_mut() {
            let (raw, rate) = pipe.buf.drain();
            if raw.is_empty() && !stopping {
                continue;
            }
            let pcm = if rate == 0 {
                Vec::new()
            } else {
                crate::voice::resample_to_16k(&raw, rate)
            };

            // Wall-anchor the timeline: the drained samples end ~now, so if
            // the channel's sample count sits far behind the wall clock the
            // source wasn't delivering (tap before any app plays audio,
            // session rebuild) — jump forward instead of compacting time.
            let mut segments = Vec::new();
            if !pcm.is_empty() {
                let wall_ms = epoch_ms().saturating_sub(args.started_epoch_ms);
                let pcm_ms = (pcm.len() / (SR / 1000)) as u64;
                let stream_end = pipe.chunker.pos_ms() + pcm_ms;
                if wall_ms > stream_end + GAP_SLACK_MS {
                    let gap = wall_ms - stream_end;
                    pipe.chunker.advance_gap(gap, &mut segments);
                    if let Some(w) = pipe.wav.as_mut() {
                        for _ in 0..(gap.min(WAV_GAP_CAP_MS) as usize * (SR / 1000)) {
                            let _ = w.write_sample(0i16);
                        }
                    }
                    eprintln!(
                        "[noted] {}: {}s delivery gap bridged",
                        pipe.name,
                        gap / 1000
                    );
                }
            }
            if let Some(w) = pipe.wav.as_mut() {
                for s in &pcm {
                    let _ = w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16);
                }
            }
            segments.extend(pipe.chunker.push(&pcm));
            if stopping {
                segments.extend(pipe.chunker.flush());
            }
            for seg in segments {
                if !looks_like_speech(&seg.samples) {
                    eprintln!(
                        "[noted] {}: non-speech segment skipped ({} ms)",
                        pipe.name,
                        seg.t1_ms - seg.t0_ms
                    );
                    continue;
                }
                let text = match transcriber.transcribe(&seg.samples) {
                    Ok(t) => apply_vocab(&t, &args.vocab),
                    Err(e) => {
                        eprintln!("[noted] segment transcribe error: {e}");
                        continue;
                    }
                };
                if is_junk(&text) {
                    continue;
                }
                if repeated_short_decode(
                    pipe.last_decode.as_ref().map(|(t, end)| (t.as_str(), *end)),
                    &text,
                    seg.t0_ms,
                ) {
                    eprintln!(
                        "[noted] {}: repeated short decode skipped: {text}",
                        pipe.name
                    );
                    continue;
                }
                if pipe.name == "me"
                    && is_echo(&text, &overlapping_text(&recent_them, seg.t0_ms, seg.t1_ms))
                {
                    eprintln!(
                        "[noted] echo suppressed: {}",
                        text.chars().take(60).collect::<String>()
                    );
                    pipe.last_decode = Some((text, seg.t1_ms));
                    continue;
                }
                let mut them_emb: Option<Vec<f32>> = None;
                if pipe.name == "them" {
                    if let Some(embedder) = embedder.as_mut() {
                        them_emb = embedder.embed(&seg.samples);
                    }
                }
                let seg_id = {
                    let state = app.state::<Db>();
                    let conn = state.0.lock().unwrap();
                    store::insert_segment_with_voice_time(
                        &conn,
                        args.meeting_id,
                        pipe.name,
                        seg.t0_ms,
                        seg.t1_ms,
                        Some(seg.voiced_ms),
                        &text,
                    )
                };
                let Ok(id) = seg_id else { continue };
                pipe.last_decode = Some((text.clone(), seg.t1_ms));
                let _ = app.emit(
                    "meeting-segment",
                    json!({
                        "meetingId": args.meeting_id,
                        "id": id,
                        "channel": pipe.name,
                        "t0_ms": seg.t0_ms,
                        "t1_ms": seg.t1_ms,
                        "voiced_ms": seg.voiced_ms,
                        "text": text,
                    }),
                );
                let recent = RecentSeg {
                    id,
                    t0: seg.t0_ms,
                    t1: seg.t1_ms,
                    text,
                };
                if pipe.name == "them" {
                    if let Some(emb) = them_emb {
                        voice_prints.push(super::diarize::SegEmb {
                            seg_id: id,
                            dur_ms: seg.t1_ms - seg.t0_ms,
                            emb,
                        });
                        prints_since_relabel += 1;
                    }
                    recent_them.retain(|r| r.t1 >= seg.t1_ms - 90_000);
                    recent_them.push(recent);

                    // The quieter mic copy may close first. Re-check recently
                    // inserted mic rows whenever the matching system segment
                    // arrives, then remove the duplicate from both DB and UI.
                    let mut echoed = Vec::new();
                    for (i, mic) in recent_me.iter().enumerate() {
                        if mic.t1 >= seg.t0_ms - ECHO_SLACK_MS
                            && mic.t0 <= seg.t1_ms + ECHO_SLACK_MS
                            && is_echo(&mic.text, &overlapping_text(&recent_them, mic.t0, mic.t1))
                        {
                            echoed.push(i);
                        }
                    }
                    for &i in echoed.iter().rev() {
                        let mic = recent_me.remove(i);
                        eprintln!(
                            "[noted] echo suppressed (late): {}",
                            mic.text.chars().take(60).collect::<String>()
                        );
                        let removed = {
                            let state = app.state::<Db>();
                            let conn = state.0.lock().unwrap();
                            store::delete_segment(&conn, mic.id)
                        };
                        if removed.is_ok() {
                            let _ = app.emit(
                                "meeting-segment-removed",
                                json!({ "meetingId": args.meeting_id, "id": mic.id }),
                            );
                        }
                    }
                } else {
                    recent_me.retain(|r| r.t1 >= seg.t1_ms - 90_000);
                    recent_me.push(recent);
                }
            }
        }
        // Provisional live labels: full-context clustering is cheap at meeting
        // scale, so rerun it as voices accumulate and stream the labels out —
        // the transcript shows who's talking DURING the call instead of "Them"
        // until stop. The stop pass below stays authoritative (echo deletion,
        // meeting_speakers rows, and profile writes happen only there; it
        // resets the channel first so no provisional label survives it).
        if !stopping && prints_since_relabel >= LIVE_RELABEL_EVERY {
            prints_since_relabel = 0;
            let labels = provisional_labels(&voice_prints);
            if !labels.is_empty() {
                {
                    let state = app.state::<Db>();
                    let conn = state.0.lock().unwrap();
                    let _ = store::set_segment_speakers(&conn, &labels);
                }
                let _ = app.emit(
                    "meeting-speakers-updated",
                    json!({
                        "meetingId": args.meeting_id,
                        "labels": labels
                            .iter()
                            .map(|(id, l)| json!({ "id": id, "label": l }))
                            .collect::<Vec<_>>(),
                    }),
                );
            }
        }
        if stopping {
            break;
        }
        std::thread::sleep(Duration::from_millis(400));
    }

    // Full-context speaker clustering; lands before stop() emits
    // meeting-stopped, so the reloaded transcript and summary see stable,
    // neutral groups. Real names remain manual except for calendar-grounded 1:1s.
    {
        let state = app.state::<Db>();
        let conn = state.0.lock().unwrap();
        finalize_speakers(
            &conn,
            args.meeting_id,
            &voice_prints,
            &args.attendees,
            args.audio_dir.as_deref(),
        );
    }

    for pipe in pipes {
        if let Some(w) = pipe.wav {
            let _ = w.finalize();
        }
    }
    transcriber.finish();
}

/// Recluster for live labels after this many new voice embeddings — often
/// enough to feel live, rare enough that whisper never waits on it.
const LIVE_RELABEL_EVERY: usize = 4;

/// Cluster the current meeting into neutral live labels. No cross-meeting
/// voiceprint or attendee name is consulted; real names are manual only.
fn provisional_labels(voice_prints: &[super::diarize::SegEmb]) -> Vec<(i64, String)> {
    if voice_prints.len() < 2 {
        return Vec::new();
    }
    let clusters = super::diarize::cluster(voice_prints);
    let named = super::diarize::assign_names(clusters);
    let mut labels = Vec::new();
    for s in &named {
        if let Some(l) = &s.label {
            labels.extend(s.seg_ids.iter().map(|&id| (id, l.clone())));
        }
    }
    labels
}

/// Full-context diarization for one meeting. It only separates voices into
/// neutral labels ("Speaker N" or "Them"); the user assigns real names.
pub fn finalize_speakers(
    conn: &rusqlite::Connection,
    meeting_id: i64,
    voice_prints: &[super::diarize::SegEmb],
    attendees: &[String],
    diagnostic_dir: Option<&Path>,
) -> usize {
    let clusters = super::diarize::cluster_with_expected(voice_prints, Some(attendees.len()));
    // Provisional live labels may disagree with the final clustering (a voice
    // that merged away, an echo cluster) — reset the channel and rewrite from
    // the full-context result so no stale label survives.
    let _ = store::clear_them_speakers(conn, meeting_id);
    let _ = store::clear_meeting_speakers(conn, meeting_id);
    if let Some(dir) = diagnostic_dir {
        write_diarization_diagnostic(dir, meeting_id, attendees, voice_prints, &clusters);
    }
    if attendees.len() == 1 {
        let attendee = attendees[0].trim();
        if attendee.is_empty() {
            return 0;
        }
        // In a true two-person meeting the system-audio channel has exactly
        // one possible human speaker. A noisy embedding can split that person
        // into several clusters; calendar identity is stronger evidence here.
        if !clusters.is_empty() {
            let mut merged = clusters[0].centroid.clone();
            let mut samples = clusters[0].seg_ids.len() as i64;
            for cluster in clusters.iter().skip(1) {
                let next_samples = cluster.seg_ids.len() as i64;
                merged = super::diarize::merge_centroid(
                    &merged,
                    samples,
                    &cluster.centroid,
                    next_samples,
                );
                samples += next_samples;
            }
            let _ = store::save_meeting_speakers(
                conn,
                meeting_id,
                &[(attendee.to_string(), merged, samples)],
            );
        }
        match store::set_one_on_one_speaker(conn, meeting_id, attendee) {
            Ok(()) => {
                eprintln!("[noted] diarization: one-on-one remote speaker labeled {attendee}")
            }
            Err(error) => eprintln!("[noted] one-on-one speaker write failed: {error}"),
        }
        return if store::them_segment_times(conn, meeting_id)
            .map(|segments| !segments.is_empty())
            .unwrap_or(false)
        {
            1
        } else {
            0
        };
    }
    if clusters.is_empty() {
        return 0;
    }
    let named = super::diarize::assign_names(clusters);
    let mut labels: Vec<(i64, String)> = Vec::new();
    let mut rows: Vec<(String, Vec<f32>, i64)> = Vec::new();
    for s in &named {
        if let Some(l) = &s.label {
            labels.extend(s.seg_ids.iter().map(|&id| (id, l.clone())));
        }
        rows.push((
            s.label.clone().unwrap_or_else(|| "Them".into()),
            s.centroid.clone(),
            s.seg_ids.len() as i64,
        ));
    }
    let _ = store::save_meeting_speakers(conn, meeting_id, &rows);
    match store::set_segment_speakers(conn, &labels) {
        Ok(()) => eprintln!(
            "[noted] diarization: {} voices, {} segments labeled",
            named.len(),
            labels.len()
        ),
        Err(e) => eprintln!("[noted] diarization write failed: {e}"),
    }
    named.len()
}

/// Keep enough local evidence to tune diarization against a real failure
/// without retaining model tensors or sending meeting content anywhere.
fn write_diarization_diagnostic(
    dir: &Path,
    meeting_id: i64,
    attendees: &[String],
    voice_prints: &[super::diarize::SegEmb],
    clusters: &[super::diarize::SpeakerCluster],
) {
    let duration = |ids: &[i64]| -> i64 {
        ids.iter()
            .filter_map(|id| voice_prints.iter().find(|s| s.seg_id == *id))
            .map(|s| s.dur_ms)
            .sum()
    };
    let lone = clusters.len() == 1;
    let rows = clusters
        .iter()
        .enumerate()
        .map(|(index, cluster)| {
            json!({
                "label": if lone { "Them".to_string() } else { format!("Speaker {}", index + 1) },
                "segment_count": cluster.seg_ids.len(),
                "duration_ms": duration(&cluster.seg_ids),
                "segment_ids": cluster.seg_ids,
            })
        })
        .collect::<Vec<_>>();
    let mut similarities = Vec::new();
    for a in 0..clusters.len() {
        for b in (a + 1)..clusters.len() {
            similarities.push(json!({
                "a": if lone { "Them".to_string() } else { format!("Speaker {}", a + 1) },
                "b": if lone { "Them".to_string() } else { format!("Speaker {}", b + 1) },
                "cosine": super::diarize::cosine(&clusters[a].centroid, &clusters[b].centroid),
            }));
        }
    }
    let report = json!({
        "version": 1,
        "meeting_id": meeting_id,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "policy": super::diarize::policy_json(),
        "invited_remote_attendees": attendees,
        "expected_remote_speakers": attendees.len(),
        "clustering": if (2..=12).contains(&attendees.len()) {
            "calendar_count_spherical_kmeans"
        } else {
            "similarity_threshold_average_linkage"
        },
        "embedded_segment_count": voice_prints.len(),
        "clusters": rows,
        "cluster_similarities": similarities,
    });
    if let Ok(bytes) = serde_json::to_vec_pretty(&report) {
        let _ = std::fs::create_dir_all(dir);
        if let Err(e) = std::fs::write(dir.join("diarization.json"), bytes) {
            eprintln!("[noted] meeting {meeting_id}: could not write diarization diagnostics: {e}");
        }
    }
}

/// Read a 16 kHz mono 16-bit WAV written by `wav_writer`, tolerating a file
/// whose header was never finalized: a crash mid-recording leaves the RIFF
/// sizes zeroed, but the PCM after the canonical 44-byte header is intact.
fn read_wav_16k(path: &Path) -> Result<Vec<f32>> {
    if let Ok(mut r) = hound::WavReader::open(path) {
        let spec = r.spec();
        if r.duration() > 0 {
            if spec.sample_rate != SR as u32
                || spec.channels != 1
                || spec.sample_format != hound::SampleFormat::Int
            {
                return Err(anyhow!("unexpected wav format in {}", path.display()));
            }
            return Ok(r
                .samples::<i16>()
                .map(|s| s.map(|v| v as f32 / 32768.0))
                .collect::<std::result::Result<Vec<_>, _>>()?);
        }
    }
    let bytes = std::fs::read(path)?;
    if bytes.len() <= 44 {
        return Ok(Vec::new());
    }
    Ok(bytes[44..]
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect())
}

/// Recovery diarization for a meeting whose worker died before the stop path
/// ran (crash / force-quit mid-recording): the live embeddings were only in
/// memory, so labels were never written and every remote line reads "Them".
/// Re-embed each them-segment from the retained wall-anchored them.wav and run
/// the exact same finalize policy as a clean stop. No-op when the meeting was
/// already diarized, the speaker model is missing, or audio wasn't retained.
pub fn rediarize_from_wav(
    conn: &rusqlite::Connection,
    model: &Path,
    meeting_dir: &Path,
    meeting_id: i64,
    force: bool,
) -> Result<usize> {
    if !force && !store::list_meeting_speakers(conn, meeting_id)?.is_empty() {
        return Ok(0); // already diarized (crash happened after the stop path)
    }
    let wav_path = meeting_dir.join("them.wav");
    if !wav_path.exists() {
        return Ok(0);
    }
    let wav = read_wav_16k(&wav_path)?;
    if wav.is_empty() {
        return Ok(0);
    }
    let segs = store::them_segment_times(conn, meeting_id)?;
    if segs.is_empty() {
        return Ok(0);
    }
    let mut embedder = super::diarize::Embedder::new(model)?;
    let mut voice_prints: Vec<super::diarize::SegEmb> = Vec::new();
    for &(seg_id, t0, t1) in &segs {
        // The WAV is wall-anchored (delivery gaps are written as zeros), so
        // segment times map straight to sample offsets. Gaps beyond
        // WAV_GAP_CAP_MS are truncated in the file — anything past the end of
        // the audio is unrecoverable, skip it.
        let s0 = (t0.max(0) as usize).saturating_mul(SR / 1000);
        let s1 = (t1.max(0) as usize)
            .saturating_mul(SR / 1000)
            .min(wav.len());
        if s0 >= s1 {
            continue;
        }
        if let Some(emb) = embedder.embed(&wav[s0..s1]) {
            voice_prints.push(super::diarize::SegEmb {
                seg_id,
                dur_ms: t1 - t0,
                emb,
            });
        }
    }
    let attendees = store::get_meeting(conn, meeting_id)
        .ok()
        .map(|meeting| store::external_attendees_for_event(conn, &meeting["event_json"]))
        .unwrap_or_default();
    Ok(finalize_speakers(
        conn,
        meeting_id,
        &voice_prints,
        &attendees,
        Some(meeting_dir),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(ms: usize, amp: f32) -> Vec<f32> {
        (0..ms * (SR / 1000))
            .map(|i| amp * (i as f32 * 0.3).sin())
            .collect()
    }
    fn quiet(ms: usize) -> Vec<f32> {
        vec![0.0; ms * (SR / 1000)]
    }

    // Deterministic "voice" embedding, mirroring diarize.rs's test helper.
    fn voice(base: usize, jitter: u64) -> Vec<f32> {
        let mut v = vec![0.0f32; 8];
        v[base] = 1.0;
        let mut x = jitter.wrapping_mul(6364136223846793005).wrapping_add(1);
        for (i, s) in v.iter_mut().enumerate() {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(i as u64);
            *s += ((x >> 33) as f32 / u32::MAX as f32 - 0.25) * 0.2;
        }
        v
    }

    fn vp(id: i64, base: usize, jitter: u64) -> super::super::diarize::SegEmb {
        super::super::diarize::SegEmb {
            seg_id: id,
            dur_ms: 5_000,
            emb: voice(base, jitter),
        }
    }

    #[test]
    fn provisional_labels_name_multiple_voices_live() {
        let prints = vec![vp(1, 0, 1), vp(2, 1, 2), vp(3, 0, 3), vp(4, 1, 4)];
        let labels = provisional_labels(&prints);
        let by_id: std::collections::HashMap<i64, String> = labels.into_iter().collect();
        assert_eq!(by_id.get(&1).map(String::as_str), Some("Speaker 1"));
        assert_eq!(by_id.get(&2).map(String::as_str), Some("Speaker 2"));
        assert_eq!(by_id.get(&3).map(String::as_str), Some("Speaker 1"));
    }

    #[test]
    fn provisional_labels_keep_lone_voice_unlabeled() {
        let prints = vec![vp(1, 2, 1), vp(2, 2, 2), vp(3, 2, 3)];
        // lone unknown voice: no live label (channel default "Them" reads better)
        assert!(provisional_labels(&prints).is_empty());
    }

    #[test]
    fn one_on_one_finalization_uses_the_sole_attendee_for_every_remote_line() {
        let tmp = std::env::temp_dir().join(format!(
            "noted_one_on_one_finalize_{}_{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&tmp);
        let conn = crate::db::init(&tmp).unwrap();
        let meeting_id = store::create_meeting_with_asr(
            &conn,
            "Brian/Edison",
            None,
            None,
            "whisper",
            "ggml-large-v3-turbo.bin",
            "2026-08-03T15:00:00Z",
        )
        .unwrap();
        let detail = store::get_meeting(&conn, meeting_id).unwrap();
        assert_eq!(detail["asr_engine"], "whisper");
        assert_eq!(detail["asr_model"], "ggml-large-v3-turbo.bin");
        let first = store::insert_segment(&conn, meeting_id, "them", 0, 5_000, "first").unwrap();
        let second =
            store::insert_segment(&conn, meeting_id, "them", 6_000, 11_000, "second").unwrap();
        let third =
            store::insert_segment(&conn, meeting_id, "them", 12_000, 17_000, "third").unwrap();
        let prints = vec![vp(first, 0, 1), vp(second, 1, 2), vp(third, 0, 3)];

        assert_eq!(
            finalize_speakers(&conn, meeting_id, &prints, &["Brian".into()], None),
            1
        );
        let labels: Vec<String> = conn
            .prepare(
                "SELECT DISTINCT speaker FROM meeting_segments
                 WHERE meeting_id = ?1 AND channel = 'them'",
            )
            .unwrap()
            .query_map([meeting_id], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(labels, vec!["Brian"]);
        let speakers = store::list_meeting_speakers(&conn, meeting_id).unwrap();
        assert_eq!(speakers.len(), 1);
        assert_eq!(speakers[0]["label"], "Brian");
        assert_eq!(speakers[0]["seg_count"], 3);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn chunker_splits_speech_by_silence() {
        let mut c = Chunker::new(ChunkerCfg::default());
        let mut audio = quiet(500);
        audio.extend(tone(1200, 0.2)); // speech A
        audio.extend(quiet(1000)); // gap closes A
        audio.extend(tone(800, 0.2)); // speech B
        let mut segs = c.push(&audio);
        segs.extend(c.flush()); // B closes at end of stream
        assert_eq!(segs.len(), 2, "two utterances split by the gap");
        // A starts around 500ms (minus preroll), well before 1000ms.
        assert!(
            segs[0].t0_ms >= 250 && segs[0].t0_ms <= 600,
            "t0={}",
            segs[0].t0_ms
        );
        assert!(segs[0].t1_ms > segs[0].t0_ms + 1000);
        // B starts after the gap.
        assert!(segs[1].t0_ms >= 2500, "t0={}", segs[1].t0_ms);
    }

    #[test]
    fn chunker_ignores_pure_silence_and_blips() {
        let mut c = Chunker::new(ChunkerCfg::default());
        let mut audio = quiet(3000);
        audio.extend(tone(60, 0.2)); // 60ms blip: shorter than open_frames*30
        audio.extend(quiet(2000));
        let mut segs = c.push(&audio);
        segs.extend(c.flush());
        assert!(
            segs.is_empty(),
            "silence and sub-90ms blips produce nothing"
        );
    }

    #[test]
    fn chunker_caps_runaway_segments() {
        let cfg = ChunkerCfg {
            max_segment_ms: 2000,
            ..ChunkerCfg::default()
        };
        let mut c = Chunker::new(cfg);
        let segs = c.push(&tone(5000, 0.2)); // 5s continuous speech
        assert!(segs.len() >= 2, "long speech splits at max_segment_ms");
        assert!(segs[0].t1_ms - segs[0].t0_ms <= 2100);
    }

    #[test]
    fn junk_filter() {
        assert!(is_junk(""));
        assert!(is_junk("  [BLANK_AUDIO]  "));
        assert!(is_junk("(music) [applause]"));
        assert!(is_junk("*ding*"), "starred sound tags are junk");
        assert!(is_junk("*sad music*"), "multi-word starred tags are junk");
        assert!(is_junk("(door slams loudly)"));
        assert!(is_junk("."), "lone punctuation is junk");
        assert!(is_junk("- ."));
        assert!(!is_junk("- Cool."));
        assert!(!is_junk("(music) hello everyone"));
        assert!(!is_junk("let's ship the meeting recorder"));
    }

    #[test]
    fn repeated_short_decode_keeps_first_and_suppresses_immediate_loop() {
        assert!(repeated_short_decode(
            Some(("Thank you.", 1_000)),
            "- thank you",
            2_000
        ));
        assert!(!repeated_short_decode(
            Some(("Thank you.", 1_000)),
            "thank you",
            40_000
        ));
        assert!(!repeated_short_decode(
            Some(("we should ship the new recorder", 1_000)),
            "we should ship the new recorder",
            2_000,
        ));
    }

    #[test]
    fn speech_gate_rejects_rumble_passes_speech() {
        let sine = |hz: f32, secs: f32, amp: f32| -> Vec<f32> {
            (0..(secs * SR as f32) as usize)
                .map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / SR as f32).sin())
                .collect()
        };
        // Low-frequency junk: mains-hum territory and below. Loud enough to
        // open the energy gate, but nothing like a voice.
        assert!(!looks_like_speech(&sine(13.0, 2.0, 0.05)));
        assert!(!looks_like_speech(&sine(45.0, 2.0, 0.3)));
        // Voiced-range tones and up: crossing density is speech-plausible.
        assert!(looks_like_speech(&sine(120.0, 2.0, 0.05)));
        assert!(looks_like_speech(&sine(300.0, 2.0, 0.05)));
        // Speech-shaped mix: an F0 with strong upper harmonics.
        let voiced: Vec<f32> = sine(110.0, 1.0, 0.04)
            .iter()
            .zip(sine(880.0, 1.0, 0.02))
            .map(|(a, b)| a + b)
            .collect();
        assert!(looks_like_speech(&voiced));
        // Silence and sub-floor noise never pass.
        assert!(!looks_like_speech(&vec![0.0; SR]));
        assert!(!looks_like_speech(&sine(500.0, 1.0, 0.002)));
        // Regression: meeting 5's broken tap stream — interleaved dual-mono
        // stereo misread as mono (adjacent duplicate samples on a slow ramp).
        // Whisper turned ~35 of these into "Thank you."
        let ramp: Vec<f32> = (0..SR * 2)
            .flat_map(|i| {
                let v = 0.02 * (2.0 * std::f32::consts::PI * 6.5 * i as f32 / SR as f32).sin();
                [v, v]
            })
            .collect();
        assert!(!looks_like_speech(&ramp));
    }

    #[test]
    fn vocab_hint_composes_and_caps() {
        assert_eq!(vocab_hint(&[], &[]), None);
        let h = vocab_hint(
            &["Mayan".into(), "  ".into(), "mayan".into()],
            &["a16z".into(), "Tauri".into()],
        )
        .unwrap();
        assert_eq!(h, "Participants: Mayan. Vocabulary: a16z, Tauri.");
        // Over-long hints keep the tail (the vocabulary), never the front.
        let many: Vec<String> = (0..200).map(|i| format!("Person{i}")).collect();
        let h = vocab_hint(&many, &["a16z".into()]).unwrap();
        assert!(h.len() <= HINT_CAP_CHARS);
        assert!(h.ends_with("Vocabulary: a16z."), "tail survives: {h}");
    }

    #[test]
    fn vocab_canonicalizes_near_miss_spellings() {
        let v = vec!["a16z".into(), "Vanta".into(), "SOC 2".into()];
        // Split renditions collapse to the canonical form.
        assert_eq!(
            apply_vocab("we talked to a 16 z yesterday", &v),
            "we talked to a16z yesterday"
        );
        assert_eq!(apply_vocab("A16-Z passed on it", &v), "a16z passed on it");
        // Case normalization on a single word.
        assert_eq!(
            apply_vocab("vanta is doing our soc 2", &v),
            "Vanta is doing our SOC 2"
        );
        // Whole words only — no rewriting inside longer words.
        assert_eq!(
            apply_vocab("advantage stays put", &v),
            "advantage stays put"
        );
        // Runs never jump sentence boundaries.
        assert_eq!(
            apply_vocab("plan A. 16 z is not a term here", &v),
            "plan A. 16 z is not a term here"
        );
        // Untouched text round-trips byte-for-byte.
        assert_eq!(
            apply_vocab("nothing to see here.", &v),
            "nothing to see here."
        );
        assert_eq!(apply_vocab("", &v), "");
    }

    #[test]
    fn vocab_short_multiword_matches_are_rejected() {
        let v = vec!["AI".into()];
        // Single word "ai" is canonicalized...
        assert_eq!(apply_vocab("the ai stuff", &v), "the AI stuff");
        // ...but "a i" (two words collapsing to 2 chars) is not.
        assert_eq!(
            apply_vocab("give it a i mean look", &v),
            "give it a i mean look"
        );
    }

    #[test]
    fn echo_detects_duplicated_remote_speech() {
        // Regression from meeting 24: the exact same remote sentence was
        // stored at 38:24 as both "Speaker 3" and "Me".
        assert!(is_echo(
            "I don't think so. Um what did the protein company say?",
            "I don't think so. Um what did the protein company say?"
        ));
        // Word-for-word duplicate (mic heard the speakers clearly).
        assert!(is_echo(
            "Sounds good. I might build it over the weekend too.",
            "Sounds good. I might build it over the weekend too. That sounds fun."
        ));
        // Real case from a standup: whisper mangles the echo (hallucinated
        // "good good…", missing words) but most mic words match.
        assert!(is_echo(
            "good good good good good he's joining sir uh did you get decent sleep this weekend",
            "He's joining soon. Did you get decent sleep this weekend? I was gonna ask you about your sleep score."
        ));
        // Overlapping-but-different speech stays (user talking over the call).
        assert!(!is_echo(
            "What up? Hey. We got a lady. Hi.",
            "What up? We got a lot of meat."
        ));
        // Short acknowledgments are never suppressed — both sides say them.
        assert!(!is_echo("Thank you.", "Thank you."));
        assert!(!is_echo("Yeah.", "Yeah, that's good."));
        // Nothing on the system channel at that time → keep.
        assert!(!is_echo("I created my own note taker this weekend", ""));
    }

    #[test]
    fn echo_matching_uses_time_overlap_and_combines_remote_chunks() {
        let remote = vec![
            RecentSeg {
                id: 1,
                t0: 37_000,
                t1: 38_500,
                text: "I don't think so.".into(),
            },
            RecentSeg {
                id: 2,
                t0: 38_500,
                t1: 41_000,
                text: "Um what did the protein company say?".into(),
            },
            RecentSeg {
                id: 3,
                t0: 80_000,
                t1: 82_000,
                text: "Unrelated later sentence".into(),
            },
        ];
        let matched = overlapping_text(&remote, 38_000, 40_500);
        assert!(is_echo(
            "I don't think so. Um what did the protein company say?",
            &matched
        ));
        assert!(!matched.contains("Unrelated later sentence"));
    }

    #[test]
    fn gap_advance_wall_aligns_the_timeline() {
        let mut c = Chunker::new(ChunkerCfg::default());
        let mut out = Vec::new();
        // Source silent for the first 73s (tap before the call app plays).
        c.advance_gap(73_000, &mut out);
        assert!(out.is_empty());
        let mut segs = c.push(&tone(1200, 0.2));
        segs.extend(c.flush());
        assert_eq!(segs.len(), 1);
        assert!(
            segs[0].t0_ms >= 72_800,
            "t0={} should sit at ~73s",
            segs[0].t0_ms
        );
    }

    #[test]
    fn gap_advance_closes_an_open_segment() {
        let mut c = Chunker::new(ChunkerCfg::default());
        let mut out = Vec::new();
        out.extend(c.push(&tone(1500, 0.2))); // speech, still open
        c.advance_gap(10_000, &mut out); // delivery stops mid-utterance
        assert_eq!(out.len(), 1, "the open segment closes at the gap");
        assert!(out[0].t1_ms <= 1_600);
        let pos = c.pos_ms();
        assert!(pos >= 11_400, "timeline jumped past the gap: {pos}");
    }

    #[test]
    fn timeline_is_frame_accurate_across_pushes() {
        // Same audio pushed in odd-sized chops must give identical segments.
        let mut audio = quiet(400);
        audio.extend(tone(900, 0.2));
        audio.extend(quiet(1000));

        let mut whole = Chunker::new(ChunkerCfg::default());
        let mut a = whole.push(&audio);
        a.extend(whole.flush());

        let mut chopped = Chunker::new(ChunkerCfg::default());
        let mut b = Vec::new();
        for chunk in audio.chunks(333) {
            b.extend(chopped.push(chunk));
        }
        b.extend(chopped.flush());

        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.t0_ms, y.t0_ms);
            assert_eq!(x.t1_ms, y.t1_ms);
        }
    }
}
