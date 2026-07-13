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

/// Whisper emits bracketed sound tags on non-speech ("[BLANK_AUDIO]", "(music)",
/// "*ding*") and lone punctuation ("."). A segment that is only such tokens
/// carries no words.
pub fn is_junk(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    t.split_whitespace().all(|w| {
        (w.starts_with('[') && w.ends_with(']'))
            || (w.starts_with('(') && w.ends_with(')'))
            || (w.starts_with('*') && w.ends_with('*'))
            || !w.chars().any(|c| c.is_alphanumeric())
    })
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

/// How far apart matching segments may sit across channels: chunk boundaries
/// differ (the echo is quieter, so the mic's gate opens later and closes
/// earlier) but the audio itself is simultaneous once timelines are
/// wall-anchored.
const ECHO_SLACK_MS: i64 = 2_500;

struct RecentSeg {
    id: i64,
    t0: i64,
    t1: i64,
    text: String,
}

/// Concatenated text of recent segments overlapping [t0, t1] ± slack.
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
    /// Wall anchor for both channel timelines (gap insertion keys off it).
    pub started_epoch_ms: u64,
    /// Speaker-embedding model, when downloaded — None = no diarization.
    pub speaker_model: Option<PathBuf>,
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
    let mut embedder = args.speaker_model.as_ref().and_then(|p| {
        super::diarize::Embedder::new(p)
            .map_err(|e| eprintln!("[noted] speaker embeddings unavailable: {e}"))
            .ok()
    });
    let mut voice_prints: Vec<super::diarize::SegEmb> = Vec::new();

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
    // "them" first: it's the clean digital copy, so its segments are already
    // recorded by the time the mic's echo rendition of the same speech closes.
    let mut pipes = [
        ChannelPipe {
            name: "them",
            buf: args.them.clone(),
            chunker: Chunker::new(ChunkerCfg::default()),
            wav: them_wav,
        },
        ChannelPipe {
            name: "me",
            buf: args.me.clone(),
            chunker: Chunker::new(ChunkerCfg::default()),
            wav: me_wav,
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
                    eprintln!("[noted] {}: {}s delivery gap bridged", pipe.name, gap / 1000);
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
                let text = match transcriber.transcribe(&seg.samples) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("[noted] segment transcribe error: {e}");
                        continue;
                    }
                };
                if is_junk(&text) {
                    continue;
                }
                if pipe.name == "me"
                    && is_echo(&text, &overlapping_text(&recent_them, seg.t0_ms, seg.t1_ms))
                {
                    eprintln!("[noted] echo suppressed: {}", text.chars().take(60).collect::<String>());
                    continue;
                }
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
                let Ok(id) = seg_id else { continue };
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
                let recent = RecentSeg { id, t0: seg.t0_ms, t1: seg.t1_ms, text };
                if pipe.name == "them" {
                    if let Some(embedder) = embedder.as_mut() {
                        if let Some(emb) = embedder.embed(&seg.samples) {
                            voice_prints.push(super::diarize::SegEmb {
                                seg_id: id,
                                dur_ms: seg.t1_ms - seg.t0_ms,
                                emb,
                            });
                        }
                    }
                    recent_them.retain(|r| r.t1 >= seg.t1_ms - 90_000);
                    recent_them.push(recent);
                    // A mic segment can close before its system-audio source
                    // does (the echo is quieter, so the mic's gate shuts
                    // early) — re-check recent mic rows against this segment.
                    let mut echoed: Vec<usize> = Vec::new();
                    for (i, m) in recent_me.iter().enumerate() {
                        if m.t1 >= seg.t0_ms - ECHO_SLACK_MS
                            && m.t0 <= seg.t1_ms + ECHO_SLACK_MS
                            && is_echo(&m.text, &overlapping_text(&recent_them, m.t0, m.t1))
                        {
                            echoed.push(i);
                        }
                    }
                    for &i in echoed.iter().rev() {
                        let m = recent_me.remove(i);
                        eprintln!(
                            "[noted] echo suppressed (late): {}",
                            m.text.chars().take(60).collect::<String>()
                        );
                        let removed = {
                            let state = app.state::<Db>();
                            let conn = state.0.lock().unwrap();
                            store::delete_segment(&conn, m.id)
                        };
                        if removed.is_ok() {
                            let _ = app.emit(
                                "meeting-segment-removed",
                                json!({ "meetingId": args.meeting_id, "id": m.id }),
                            );
                        }
                    }
                } else {
                    recent_me.retain(|r| r.t1 >= seg.t1_ms - 90_000);
                    recent_me.push(recent);
                }
            }
        }
        if stopping {
            break;
        }
        std::thread::sleep(Duration::from_millis(400));
    }

    // Full-context speaker clustering; lands before stop() emits
    // meeting-stopped, so the reloaded transcript and the summary see names.
    if !voice_prints.is_empty() {
        let labels = super::diarize::cluster(&voice_prints);
        if !labels.is_empty() {
            let speakers = labels
                .iter()
                .map(|(_, l)| l.as_str())
                .collect::<std::collections::HashSet<_>>()
                .len();
            let state = app.state::<Db>();
            let conn = state.0.lock().unwrap();
            match store::set_segment_speakers(&conn, &labels) {
                Ok(()) => eprintln!(
                    "[noted] diarization: {speakers} speakers across {} segments",
                    labels.len()
                ),
                Err(e) => eprintln!("[noted] diarization write failed: {e}"),
            }
        }
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
        assert!(is_junk("*ding*"), "starred sound tags are junk");
        assert!(is_junk("."), "lone punctuation is junk");
        assert!(is_junk("- ."));
        assert!(!is_junk("- Cool."));
        assert!(!is_junk("let's ship the meeting recorder"));
    }

    #[test]
    fn echo_detects_duplicated_remote_speech() {
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
    fn gap_advance_wall_aligns_the_timeline() {
        let mut c = Chunker::new(ChunkerCfg::default());
        let mut out = Vec::new();
        // Source silent for the first 73s (tap before the call app plays).
        c.advance_gap(73_000, &mut out);
        assert!(out.is_empty());
        let mut segs = c.push(&tone(1200, 0.2));
        segs.extend(c.flush());
        assert_eq!(segs.len(), 1);
        assert!(segs[0].t0_ms >= 72_800, "t0={} should sit at ~73s", segs[0].t0_ms);
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
