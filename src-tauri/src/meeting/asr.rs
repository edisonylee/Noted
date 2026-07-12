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
use std::sync::Arc;
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
            max_segment_ms: 25_000,
            min_segment_ms: 400,
        }
    }
}

pub struct PendingSegment {
    pub t0_ms: i64,
    pub t1_ms: i64,
    pub samples: Vec<f32>,
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
            out.push(PendingSegment {
                t0_ms,
                t1_ms: t0_ms + seg_ms as i64,
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
}

// ---------------------------------------------------------------------------
// Whisper
// ---------------------------------------------------------------------------

/// Model stays loaded for the whole meeting; a fresh (cheap) state per segment
/// avoids self-referential lifetime knots. Calls are serialized by the worker
/// loop so Metal never sees two decodes at once.
pub struct Transcriber {
    ctx: whisper_rs::WhisperContext,
}

impl Transcriber {
    pub fn new(model_path: &Path) -> Result<Self> {
        let path = model_path
            .to_str()
            .ok_or_else(|| anyhow!("bad model path"))?;
        let ctx = whisper_rs::WhisperContext::new_with_params(
            path,
            whisper_rs::WhisperContextParameters::default(),
        )
        .map_err(|e| anyhow!("whisper load failed: {e:?}"))?;
        Ok(Self { ctx })
    }

    pub fn transcribe(&self, samples: &[f32]) -> Result<String> {
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| anyhow!("whisper state: {e:?}"))?;
        let mut params =
            whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_print_progress(false);
        params.set_print_special(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
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
}

/// Whisper emits bracketed sound tags on non-speech ("[BLANK_AUDIO]", "(music)").
/// A segment that is only such tags carries no words.
pub fn is_junk(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    let only_tags = t
        .split_whitespace()
        .all(|w| (w.starts_with('[') && w.ends_with(']')) || (w.starts_with('(') && w.ends_with(')')));
    only_tags
}

// ---------------------------------------------------------------------------
// Worker: the per-meeting loop tying capture → chunker → whisper → DB/UI.
// ---------------------------------------------------------------------------

struct ChannelPipe {
    name: &'static str,
    buf: Arc<ChannelBuf>,
    chunker: Chunker,
    wav: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>,
}

fn wav_writer(path: &Path) -> Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SR as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    hound::WavWriter::create(path, spec).ok()
}

pub struct WorkerArgs {
    pub meeting_id: i64,
    pub me: Arc<ChannelBuf>,
    pub them: Arc<ChannelBuf>,
    pub stop: Arc<AtomicBool>,
    pub model_path: PathBuf,
    /// None = audio retention off. Some(dir) = write me.wav / them.wav there.
    pub audio_dir: Option<PathBuf>,
}

pub fn run_worker(app: tauri::AppHandle, args: WorkerArgs) {
    let transcriber = match Transcriber::new(&args.model_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[noted] meeting ASR unavailable: {e}");
            let state = app.state::<Db>();
            let conn = state.0.lock().unwrap();
            let _ = store::set_status(&conn, args.meeting_id, "failed");
            return;
        }
    };

    let (me_wav, them_wav) = match &args.audio_dir {
        Some(dir) => {
            let _ = std::fs::create_dir_all(dir);
            (
                wav_writer(&dir.join("me.wav")),
                wav_writer(&dir.join("them.wav")),
            )
        }
        None => (None, None),
    };
    let mut pipes = [
        ChannelPipe {
            name: "me",
            buf: args.me.clone(),
            chunker: Chunker::new(ChunkerCfg::default()),
            wav: me_wav,
        },
        ChannelPipe {
            name: "them",
            buf: args.them.clone(),
            chunker: Chunker::new(ChunkerCfg::default()),
            wav: them_wav,
        },
    ];

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
            if let Some(w) = pipe.wav.as_mut() {
                for s in &pcm {
                    let _ = w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16);
                }
            }
            let mut segments = pipe.chunker.push(&pcm);
            if stopping {
                segments.extend(pipe.chunker.flush());
            }
            for seg in segments {
                match transcriber.transcribe(&seg.samples) {
                    Ok(text) if !is_junk(&text) => {
                        let seg_id = {
                            let state = app.state::<Db>();
                            let conn = state.0.lock().unwrap();
                            store::insert_segment(
                                &conn,
                                args.meeting_id,
                                pipe.name,
                                seg.t0_ms,
                                seg.t1_ms,
                                &text,
                            )
                        };
                        if let Ok(id) = seg_id {
                            let _ = app.emit(
                                "meeting-segment",
                                json!({
                                    "meetingId": args.meeting_id,
                                    "id": id,
                                    "channel": pipe.name,
                                    "t0_ms": seg.t0_ms,
                                    "t1_ms": seg.t1_ms,
                                    "text": text,
                                }),
                            );
                        }
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("[noted] segment transcribe error: {e}"),
                }
            }
        }
        if stopping {
            break;
        }
        std::thread::sleep(Duration::from_millis(400));
    }

    for pipe in pipes {
        if let Some(w) = pipe.wav {
            let _ = w.finalize();
        }
    }
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
        assert!(segs[0].t0_ms >= 250 && segs[0].t0_ms <= 600, "t0={}", segs[0].t0_ms);
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
        assert!(segs.is_empty(), "silence and sub-90ms blips produce nothing");
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
        assert!(!is_junk("let's ship the meeting recorder"));
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
