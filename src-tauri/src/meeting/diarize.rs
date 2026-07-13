// Speaker diarization for the "them" (system-audio) stream.
//
// The ASR worker computes one voice embedding per transcribed them-segment as
// it lands (WeSpeaker CAM++ via sherpa-onnx, 512-dim, ~30ms per segment). At
// stop, `cluster` labels ALL segments at once — full-context agglomerative
// clustering beats online assignment, needs no retained audio, and finishes
// before the meeting-stopped event, so the UI reload and the summary both see
// the names. During recording the transcript shows the channel default
// ("Them"); labels appear the moment the meeting ends.
//
// Everything below the model is deterministic policy:
//   - only segments >= SEED_MS may found a speaker (short blips embed noisily)
//   - shorter segments snap to the nearest final centroid, or stay NULL
//   - a single cluster stays NULL everywhere — in a 1:1, "Them" reads better
//     than a lone "Speaker 1"

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

pub const MODEL_FILE: &str = "speaker-embed.onnx";

/// Segments shorter than this embed too noisily to use at all.
const MIN_EMBED_MS: i64 = 1_000;
/// Only segments at least this long can found a new speaker cluster.
const SEED_MS: i64 = 2_500;
/// Average-linkage merge floor (cosine). Tuned on a real multi-speaker
/// standup: within-speaker links run 0.8+, cross-speaker below ~0.5.
const CUTOFF: f32 = 0.55;
/// A short segment snaps to the nearest speaker only above this.
const SNAP_MIN: f32 = 0.30;
/// Embed at most the first 12s of a segment (plenty for a voice print).
const EMBED_CAP_MS: i64 = 12_000;
/// Matrix-size guard: cluster at most this many seeds (longest kept).
const MAX_SEEDS: usize = 1_200;

pub fn model_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    let p = app
        .path()
        .app_data_dir()
        .ok()?
        .join("models")
        .join(MODEL_FILE);
    p.exists().then_some(p)
}

// ---------------------------------------------------------------------------
// Embedding (the only non-deterministic-ish part: a model forward pass)
// ---------------------------------------------------------------------------

pub struct Embedder {
    extractor: sherpa_rs::speaker_id::EmbeddingExtractor,
}

impl Embedder {
    pub fn new(model: &Path) -> Result<Self> {
        let extractor = sherpa_rs::speaker_id::EmbeddingExtractor::new(
            sherpa_rs::speaker_id::ExtractorConfig {
                model: model.to_string_lossy().to_string(),
                ..Default::default()
            },
        )
        .map_err(|e| anyhow!("speaker model load failed: {e}"))?;
        Ok(Self { extractor })
    }

    /// Voice embedding for a 16 kHz mono segment; None when too short.
    pub fn embed(&mut self, samples_16k: &[f32]) -> Option<Vec<f32>> {
        let dur_ms = (samples_16k.len() / 16) as i64;
        if dur_ms < MIN_EMBED_MS {
            return None;
        }
        let cap = (EMBED_CAP_MS as usize * 16).min(samples_16k.len());
        self.extractor
            .compute_speaker_embedding(samples_16k[..cap].to_vec(), 16_000)
            .ok()
    }
}

// ---------------------------------------------------------------------------
// Clustering (pure, unit-tested — no model involved)
// ---------------------------------------------------------------------------

pub struct SegEmb {
    pub seg_id: i64,
    pub dur_ms: i64,
    pub emb: Vec<f32>,
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

fn mean(members: &[usize], segs: &[&SegEmb]) -> Vec<f32> {
    let dim = segs[members[0]].emb.len();
    let mut m = vec![0.0f32; dim];
    for &i in members {
        for (mv, ev) in m.iter_mut().zip(&segs[i].emb) {
            *mv += ev;
        }
    }
    for mv in m.iter_mut() {
        *mv /= members.len() as f32;
    }
    m
}

/// Label segments "Speaker 1..N" (numbered by first appearance — `segs` must
/// be in timeline order). Returns only the segments that earned a label; a
/// single-cluster meeting returns nothing.
pub fn cluster(segs: &[SegEmb]) -> Vec<(i64, String)> {
    let segs: Vec<&SegEmb> = segs.iter().filter(|s| !s.emb.is_empty()).collect();
    let mut seed_idx: Vec<usize> = (0..segs.len())
        .filter(|&i| segs[i].dur_ms >= SEED_MS)
        .collect();
    if seed_idx.len() > MAX_SEEDS {
        seed_idx.sort_by_key(|&i| -segs[i].dur_ms);
        seed_idx.truncate(MAX_SEEDS);
        seed_idx.sort_unstable();
    }
    if seed_idx.len() < 2 {
        return Vec::new();
    }

    // Average-linkage agglomerative over a precomputed similarity matrix,
    // merged via Lance-Williams so no embedding is touched twice.
    let n = seed_idx.len();
    let mut sim = vec![0.0f32; n * n];
    for a in 0..n {
        for b in (a + 1)..n {
            let s = cosine(&segs[seed_idx[a]].emb, &segs[seed_idx[b]].emb);
            sim[a * n + b] = s;
            sim[b * n + a] = s;
        }
    }
    let mut members: Vec<Option<Vec<usize>>> =
        seed_idx.iter().map(|&i| Some(vec![i])).collect();
    loop {
        let mut best = (usize::MAX, usize::MAX, CUTOFF);
        for a in 0..n {
            if members[a].is_none() {
                continue;
            }
            for b in (a + 1)..n {
                if members[b].is_none() {
                    continue;
                }
                if sim[a * n + b] >= best.2 {
                    best = (a, b, sim[a * n + b]);
                }
            }
        }
        let (a, b, _) = best;
        if a == usize::MAX {
            break;
        }
        let (na, nb) = (
            members[a].as_ref().unwrap().len() as f32,
            members[b].as_ref().unwrap().len() as f32,
        );
        for c in 0..n {
            if c == a || c == b || members[c].is_none() {
                continue;
            }
            let merged = (na * sim[a * n + c] + nb * sim[b * n + c]) / (na + nb);
            sim[a * n + c] = merged;
            sim[c * n + a] = merged;
        }
        let moved = members[b].take().unwrap();
        members[a].as_mut().unwrap().extend(moved);
    }

    let mut clusters: Vec<Vec<usize>> = members.into_iter().flatten().collect();
    if clusters.len() < 2 {
        return Vec::new();
    }
    // "Speaker 1" is whoever spoke first.
    clusters.sort_by_key(|c| *c.iter().min().unwrap());
    let centroids: Vec<Vec<f32>> = clusters.iter().map(|c| mean(c, &segs)).collect();

    let mut out: Vec<(i64, String)> = Vec::new();
    let mut labeled = vec![None::<usize>; segs.len()];
    for (ci, c) in clusters.iter().enumerate() {
        for &i in c {
            labeled[i] = Some(ci);
        }
    }
    for (i, seg) in segs.iter().enumerate() {
        let ci = labeled[i].or_else(|| {
            // Non-seed: snap to the nearest final speaker, if close enough.
            let (mut bi, mut bs) = (None, SNAP_MIN);
            for (ci, cen) in centroids.iter().enumerate() {
                let s = cosine(&seg.emb, cen);
                if s >= bs {
                    bs = s;
                    bi = Some(ci);
                }
            }
            bi
        });
        if let Some(ci) = ci {
            out.push((seg.seg_id, format!("Speaker {}", ci + 1)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Deterministic "voice": a base direction plus a small per-sample jitter.
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

    fn seg(id: i64, dur: i64, base: usize, jitter: u64) -> SegEmb {
        SegEmb { seg_id: id, dur_ms: dur, emb: voice(base, jitter) }
    }

    #[test]
    fn two_speakers_separate_and_number_by_first_appearance() {
        let segs = vec![
            seg(10, 5000, 1, 1), // speaker B talks first → "Speaker 1"
            seg(11, 4000, 0, 2),
            seg(12, 6000, 1, 3),
            seg(13, 3000, 0, 4),
            seg(14, 5000, 0, 5),
        ];
        let out = cluster(&segs);
        let get = |id: i64| out.iter().find(|(i, _)| *i == id).map(|(_, l)| l.as_str());
        assert_eq!(get(10), Some("Speaker 1"));
        assert_eq!(get(12), Some("Speaker 1"));
        assert_eq!(get(11), Some("Speaker 2"));
        assert_eq!(get(13), Some("Speaker 2"));
        assert_eq!(get(14), Some("Speaker 2"));
    }

    #[test]
    fn single_speaker_stays_unlabeled() {
        let segs: Vec<SegEmb> = (0..6).map(|i| seg(i, 5000, 2, i as u64)).collect();
        assert!(cluster(&segs).is_empty(), "1:1 calls keep the channel default");
    }

    #[test]
    fn short_segment_snaps_to_nearest_speaker() {
        let segs = vec![
            seg(1, 5000, 0, 1),
            seg(2, 5000, 1, 2),
            seg(3, 1200, 0, 3), // short — can't seed, but snaps to speaker 1
            seg(4, 5000, 0, 4),
            seg(5, 5000, 1, 5),
        ];
        let out = cluster(&segs);
        let get = |id: i64| out.iter().find(|(i, _)| *i == id).map(|(_, l)| l.as_str());
        assert_eq!(get(3), get(1));
    }

    #[test]
    fn distant_short_blip_stays_unlabeled() {
        let mut segs = vec![
            seg(1, 5000, 0, 1),
            seg(2, 5000, 1, 2),
            seg(3, 5000, 0, 3),
            seg(4, 5000, 1, 4),
        ];
        // Orthogonal to both speakers (a notification chime, music, …).
        segs.push(seg(9, 1200, 3, 5));
        let out = cluster(&segs);
        assert!(out.iter().all(|(id, _)| *id != 9));
    }

    #[test]
    fn empty_and_tiny_inputs() {
        assert!(cluster(&[]).is_empty());
        assert!(cluster(&[seg(1, 5000, 0, 1)]).is_empty());
    }

    /// Tuning harness against a real recording — needs the model plus a
    /// meeting's them.wav and a `id\tt0\tt1\ttext` TSV of its them-segments:
    ///   SPEAKER_MODEL=…/speaker-embed.onnx THEM_WAV=…/them.wav SEGS_TSV=…/them.tsv \
    ///   cargo test diarize_real_meeting -- --ignored --nocapture
    #[test]
    #[ignore]
    fn diarize_real_meeting() {
        let (model, wav_path, tsv_path) = match (
            std::env::var("SPEAKER_MODEL"),
            std::env::var("THEM_WAV"),
            std::env::var("SEGS_TSV"),
        ) {
            (Ok(m), Ok(w), Ok(t)) => (m, w, t),
            _ => panic!("set SPEAKER_MODEL, THEM_WAV, SEGS_TSV"),
        };
        let mut embedder = Embedder::new(Path::new(&model)).expect("model");
        let mut reader = hound::WavReader::open(&wav_path).expect("wav");
        let wav: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect();
        let tsv = std::fs::read_to_string(&tsv_path).expect("tsv");
        let mut segs = Vec::new();
        let mut texts = std::collections::HashMap::new();
        for line in tsv.lines() {
            let mut f = line.splitn(4, '\t');
            let id: i64 = f.next().unwrap().parse().unwrap();
            let t0: i64 = f.next().unwrap().parse().unwrap();
            let t1: i64 = f.next().unwrap().parse().unwrap();
            texts.insert(id, f.next().unwrap_or("").to_string());
            let s0 = (t0 as usize * 16).min(wav.len());
            let s1 = (t1 as usize * 16).min(wav.len());
            if let Some(emb) = embedder.embed(&wav[s0..s1]) {
                segs.push(SegEmb { seg_id: id, dur_ms: t1 - t0, emb });
            }
        }
        let out = cluster(&segs);
        for (id, label) in &out {
            println!("{label}  {}", texts[id].chars().take(70).collect::<String>());
        }
        let speakers: std::collections::HashSet<_> = out.iter().map(|(_, l)| l).collect();
        println!("→ {} labeled segments, {} speakers", out.len(), speakers.len());
        assert!(speakers.len() >= 2, "a multi-speaker meeting should separate");
    }
}
