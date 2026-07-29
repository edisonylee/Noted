// Speaker diarization for the "them" (system-audio) stream.
//
// The ASR worker computes one voice embedding per transcribed them-segment as
// it lands (English ERes2Net via sherpa-onnx). At
// stop, `cluster` groups ALL segments at once — full-context agglomerative
// clustering beats online assignment, needs no retained audio, and finishes
// before the meeting-stopped event, so the UI reload and the summary both see
// the names. During recording the transcript shows the channel default
// ("Them"); labels appear the moment the meeting ends.
//
// Labeling (`assign_names`): multiple clusters become "Speaker N" by first
// appearance. A lone cluster stays NULL — in a 1:1, "Them" reads better than
// a lone "Speaker 1". Real names are assigned manually per meeting.
//
// Everything below the model is deterministic policy:
//   - only segments >= SEED_MS may found a speaker (short blips embed noisily)
//   - shorter segments snap to the nearest final centroid, or stay NULL

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

pub const MODEL_FILE: &str = "speaker-embed-eres2net.onnx";

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
/// Calendar rosters larger than this are more likely to contain mailing lists,
/// rooms, or optional invitees than a useful diarization count.
const MAX_EXPECTED_SPEAKERS: usize = 12;

pub(crate) fn policy_json() -> serde_json::Value {
    serde_json::json!({
        "min_embed_ms": MIN_EMBED_MS,
        "seed_ms": SEED_MS,
        "cluster_cutoff": CUTOFF,
        "short_segment_snap_min": SNAP_MIN,
        "embed_cap_ms": EMBED_CAP_MS,
        "max_seeds": MAX_SEEDS,
        "max_expected_speakers": MAX_EXPECTED_SPEAKERS,
        "identity_policy": "manual",
    })
}

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
// Embedding blobs (f32 little-endian) stored in meeting_speakers so labels can
// be merged within a meeting without changing its anonymous diarization.
// ---------------------------------------------------------------------------

pub fn emb_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn blob_to_emb(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// Weighted running mean used when two labels are merged within one meeting.
pub fn merge_centroid(old: &[f32], old_n: i64, cur: &[f32], cur_n: i64) -> Vec<f32> {
    let (on, cn) = (old_n.max(0) as f32, cur_n.max(1) as f32);
    old.iter()
        .zip(cur)
        .map(|(o, c)| (o * on + c * cn) / (on + cn))
        .collect()
}

// ---------------------------------------------------------------------------
// Clustering + anonymous labeling (pure, unit-tested — no model involved)
// ---------------------------------------------------------------------------

pub struct SegEmb {
    pub seg_id: i64,
    pub dur_ms: i64,
    pub emb: Vec<f32>,
}

pub struct SpeakerCluster {
    /// Timeline order (seeds first-appearance, then snapped short segments).
    pub seg_ids: Vec<i64>,
    /// Mean of the seed embeddings (short snapped segments don't move it).
    pub centroid: Vec<f32>,
}

pub struct NamedSpeaker {
    /// None = keep the channel default ("Them"): a lone unrecognized voice.
    pub label: Option<String>,
    pub seg_ids: Vec<i64>,
    pub centroid: Vec<f32>,
}

pub(crate) fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// Group segments by voice; clusters come back ordered by first appearance
/// (`segs` must be in timeline order). May return a single cluster — the
/// naming stage decides what that means.
pub fn cluster(segs: &[SegEmb]) -> Vec<SpeakerCluster> {
    cluster_with_expected(segs, None)
}

/// Cluster with an optional calendar-roster count. The roster never assigns
/// identities; it selects an exact anonymous partition when the expected
/// remote-speaker count is small and credible.
pub fn cluster_with_expected(
    segs: &[SegEmb],
    expected_speakers: Option<usize>,
) -> Vec<SpeakerCluster> {
    let expected = expected_speakers.filter(|&n| (2..=MAX_EXPECTED_SPEAKERS).contains(&n));
    match expected {
        Some(count) => cluster_exact_count(segs, count),
        None => cluster_at(segs, CUTOFF),
    }
}

/// Deterministic spherical k-means for calendar meetings where the number of
/// remote participants is known. Hierarchical thresholding can leave one huge
/// cluster plus several token outliers; an exact partition lets dense voice
/// subgroups inside that large bucket separate.
fn cluster_exact_count(segs: &[SegEmb], count: usize) -> Vec<SpeakerCluster> {
    let segs: Vec<&SegEmb> = segs.iter().filter(|seg| !seg.emb.is_empty()).collect();
    let mut seeds: Vec<usize> = (0..segs.len())
        .filter(|&index| segs[index].dur_ms >= SEED_MS)
        .collect();
    if seeds.len() > MAX_SEEDS {
        seeds.sort_by_key(|&index| -segs[index].dur_ms);
        seeds.truncate(MAX_SEEDS);
        seeds.sort_unstable();
    }
    let count = count.min(seeds.len());
    if count < 2 {
        return cluster_at(
            &segs
                .iter()
                .map(|seg| SegEmb {
                    seg_id: seg.seg_id,
                    dur_ms: seg.dur_ms,
                    emb: seg.emb.clone(),
                })
                .collect::<Vec<_>>(),
            CUTOFF,
        );
    }

    // Start in the densest part of the embedding space, then add the point
    // farthest from its nearest chosen center. This is deterministic and much
    // less likely than "first point" to spend a center on a noisy blip.
    let first = seeds
        .iter()
        .copied()
        .max_by(|&a, &b| {
            let density = |index: usize| {
                seeds
                    .iter()
                    .map(|&other| cosine(&segs[index].emb, &segs[other].emb))
                    .sum::<f32>()
            };
            density(a)
                .partial_cmp(&density(b))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.cmp(&a))
        })
        .unwrap();
    let mut centers = vec![segs[first].emb.clone()];
    while centers.len() < count {
        let next = seeds
            .iter()
            .copied()
            .min_by(|&a, &b| {
                let nearest = |index: usize| {
                    centers
                        .iter()
                        .map(|center| cosine(&segs[index].emb, center))
                        .fold(f32::NEG_INFINITY, f32::max)
                };
                nearest(a)
                    .partial_cmp(&nearest(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.cmp(&b))
            })
            .unwrap();
        centers.push(segs[next].emb.clone());
    }

    let mut assignments = vec![usize::MAX; seeds.len()];
    for _ in 0..30 {
        let next_assignments = seeds
            .iter()
            .map(|&index| {
                centers
                    .iter()
                    .enumerate()
                    .max_by(|(ai, a), (bi, b)| {
                        cosine(&segs[index].emb, a)
                            .partial_cmp(&cosine(&segs[index].emb, b))
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| bi.cmp(ai))
                    })
                    .map(|(center, _)| center)
                    .unwrap_or(0)
            })
            .collect::<Vec<_>>();
        if next_assignments == assignments {
            break;
        }
        assignments = next_assignments;
        for (center_index, center) in centers.iter_mut().enumerate() {
            let members = seeds
                .iter()
                .zip(assignments.iter())
                .filter(|(_, assigned)| **assigned == center_index)
                .map(|(&seed, _)| seed)
                .collect::<Vec<_>>();
            if members.is_empty() {
                continue;
            }
            center.fill(0.0);
            for member in &members {
                for (mean, value) in center.iter_mut().zip(segs[*member].emb.iter()) {
                    *mean += value;
                }
            }
            center
                .iter_mut()
                .for_each(|value| *value /= members.len() as f32);
        }
    }

    let mut groups = (0..count)
        .map(|center| {
            seeds
                .iter()
                .zip(assignments.iter())
                .filter(|(_, assigned)| **assigned == center)
                .map(|(&seed, _)| seed)
                .collect::<Vec<_>>()
        })
        .filter(|group| !group.is_empty())
        .collect::<Vec<_>>();
    groups.sort_by_key(|group| *group.iter().min().unwrap());
    let mut out = groups
        .into_iter()
        .map(|group| {
            let mut centroid = vec![0.0f32; segs[group[0]].emb.len()];
            for &index in &group {
                for (mean, value) in centroid.iter_mut().zip(segs[index].emb.iter()) {
                    *mean += value;
                }
            }
            centroid
                .iter_mut()
                .for_each(|value| *value /= group.len() as f32);
            SpeakerCluster {
                seg_ids: group.iter().map(|&index| segs[index].seg_id).collect(),
                centroid,
            }
        })
        .collect::<Vec<_>>();

    let seeded: std::collections::HashSet<usize> = seeds.iter().copied().collect();
    for (index, seg) in segs.iter().enumerate() {
        if seeded.contains(&index) {
            continue;
        }
        let nearest = out
            .iter()
            .enumerate()
            .map(|(cluster, candidate)| (cluster, cosine(&seg.emb, &candidate.centroid)))
            .max_by(|(ai, a), (bi, b)| {
                a.partial_cmp(b)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| bi.cmp(ai))
            });
        if let Some((cluster, similarity)) = nearest {
            if similarity >= SNAP_MIN {
                out[cluster].seg_ids.push(seg.seg_id);
            }
        }
    }
    out
}

/// `cluster` with an explicit merge floor — the tuning harness sweeps this to
/// see where a recording's voice structure actually separates.
pub fn cluster_at(segs: &[SegEmb], cutoff: f32) -> Vec<SpeakerCluster> {
    let segs: Vec<&SegEmb> = segs.iter().filter(|s| !s.emb.is_empty()).collect();
    let mut seed_idx: Vec<usize> = (0..segs.len())
        .filter(|&i| segs[i].dur_ms >= SEED_MS)
        .collect();
    if seed_idx.len() > MAX_SEEDS {
        seed_idx.sort_by_key(|&i| -segs[i].dur_ms);
        seed_idx.truncate(MAX_SEEDS);
        seed_idx.sort_unstable();
    }
    if seed_idx.is_empty() {
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
    let mut members: Vec<Option<Vec<usize>>> = seed_idx.iter().map(|&i| Some(vec![i])).collect();
    loop {
        let mut best = (usize::MAX, usize::MAX, cutoff);
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

    let mut groups: Vec<Vec<usize>> = members.into_iter().flatten().collect();
    // Order by first appearance so "Speaker 1" is whoever spoke first.
    groups.sort_by_key(|c| *c.iter().min().unwrap());

    let mut out: Vec<SpeakerCluster> = groups
        .iter()
        .map(|g| {
            let dim = segs[g[0]].emb.len();
            let mut centroid = vec![0.0f32; dim];
            for &i in g {
                for (cv, ev) in centroid.iter_mut().zip(&segs[i].emb) {
                    *cv += ev;
                }
            }
            centroid.iter_mut().for_each(|v| *v /= g.len() as f32);
            let mut ids: Vec<usize> = g.clone();
            ids.sort_unstable();
            SpeakerCluster {
                seg_ids: ids.iter().map(|&i| segs[i].seg_id).collect(),
                centroid,
            }
        })
        .collect();

    // Snap short-but-embeddable segments to the nearest cluster.
    let seeded: std::collections::HashSet<usize> = seed_idx.iter().copied().collect();
    for (i, seg) in segs.iter().enumerate() {
        if seeded.contains(&i) {
            continue;
        }
        let (mut bi, mut bs) = (None, SNAP_MIN);
        for (ci, c) in out.iter().enumerate() {
            let s = cosine(&seg.emb, &c.centroid);
            if s >= bs {
                bs = s;
                bi = Some(ci);
            }
        }
        if let Some(ci) = bi {
            out[ci].seg_ids.push(seg.seg_id);
        }
    }
    out
}

/// Turn clusters into anonymous display labels by appearance. A meeting with
/// one remote voice keeps the channel default ("Them").
pub fn assign_names(clusters: Vec<SpeakerCluster>) -> Vec<NamedSpeaker> {
    let lone = clusters.len() == 1;
    clusters
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let label = (!lone).then(|| format!("Speaker {}", i + 1));
            NamedSpeaker {
                label,
                seg_ids: c.seg_ids,
                centroid: c.centroid,
            }
        })
        .collect()
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
        SegEmb {
            seg_id: id,
            dur_ms: dur,
            emb: voice(base, jitter),
        }
    }

    fn label_of(named: &[NamedSpeaker], id: i64) -> Option<String> {
        named
            .iter()
            .find(|s| s.seg_ids.contains(&id))
            .and_then(|s| s.label.clone())
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
        let named = assign_names(cluster(&segs));
        assert_eq!(label_of(&named, 10).as_deref(), Some("Speaker 1"));
        assert_eq!(label_of(&named, 12).as_deref(), Some("Speaker 1"));
        assert_eq!(label_of(&named, 11).as_deref(), Some("Speaker 2"));
        assert_eq!(label_of(&named, 13).as_deref(), Some("Speaker 2"));
        assert_eq!(label_of(&named, 14).as_deref(), Some("Speaker 2"));
    }

    #[test]
    fn lone_unknown_voice_stays_unlabeled() {
        let segs: Vec<SegEmb> = (0..6).map(|i| seg(i, 5000, 2, i as u64)).collect();
        let named = assign_names(cluster(&segs));
        assert_eq!(named.len(), 1);
        assert!(
            named[0].label.is_none(),
            "1:1 calls keep the channel default"
        );
    }

    #[test]
    fn calendar_count_prevents_distinct_attendees_from_merging_away() {
        // At a deliberately permissive similarity floor, the default pass can
        // merge these four close voices. A four-person roster keeps four
        // anonymous clusters without assigning any attendee names.
        let segs = vec![
            seg(1, 5000, 0, 1),
            seg(2, 5000, 1, 2),
            seg(3, 5000, 2, 3),
            seg(4, 5000, 3, 4),
        ];
        let hinted = cluster_with_expected(&segs, Some(4));
        assert_eq!(hinted.len(), 4);
        let named = assign_names(hinted);
        assert_eq!(named.len(), 4);
        assert!(named.iter().all(|s| {
            s.label
                .as_deref()
                .is_some_and(|label| label.starts_with("Speaker "))
        }));
    }

    #[test]
    fn calendar_count_separates_low_talkers_from_a_dense_majority() {
        let mut segs = Vec::new();
        let mut id = 0i64;
        for (voice_index, count) in [(0, 30), (1, 12), (2, 5), (3, 1)] {
            for sample in 0..count {
                id += 1;
                segs.push(seg(id, 5_000, voice_index, (id * 100 + sample) as u64));
            }
        }
        let clusters = cluster_with_expected(&segs, Some(4));
        assert_eq!(clusters.len(), 4);
        let mut sizes = clusters
            .iter()
            .map(|cluster| cluster.seg_ids.len())
            .collect::<Vec<_>>();
        sizes.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(sizes, vec![30, 12, 5, 1]);
    }

    #[test]
    fn implausibly_large_roster_does_not_force_noise_clusters() {
        let segs: Vec<SegEmb> = (0..6).map(|i| seg(i, 5000, 2, i as u64)).collect();
        assert_eq!(cluster_with_expected(&segs, Some(99)).len(), 1);
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
        let named = assign_names(cluster(&segs));
        assert_eq!(label_of(&named, 3), label_of(&named, 1));
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
        let named = assign_names(cluster(&segs));
        assert!(label_of(&named, 9).is_none());
    }

    #[test]
    fn empty_and_tiny_inputs() {
        assert!(cluster(&[]).is_empty());
        let one = assign_names(cluster(&[seg(1, 5000, 0, 1)]));
        assert_eq!(one.len(), 1);
        assert!(one[0].label.is_none());
    }

    #[test]
    fn blob_roundtrip_and_running_mean() {
        let v = vec![0.25f32, -1.5, 3.0];
        assert_eq!(blob_to_emb(&emb_to_blob(&v)), v);
        // 3 old samples at 1.0, 1 new at 5.0 → mean 2.0
        let merged = merge_centroid(&[1.0], 3, &[5.0], 1);
        assert!((merged[0] - 2.0).abs() < 1e-6);
    }

    /// Tuning harness against a real recording — needs the model plus a
    /// meeting's them.wav and a `id\tt0\tt1\ttext` TSV of its them-segments:
    ///   SPEAKER_MODEL=…/speaker-embed-eres2net.onnx THEM_WAV=…/them.wav SEGS_TSV=…/them.tsv \
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
                segs.push(SegEmb {
                    seg_id: id,
                    dur_ms: t1 - t0,
                    emb,
                });
            }
        }
        // Where does this recording's voice structure separate? Sweep the
        // merge floor and show the pairwise-similarity distribution — the gap
        // between within-speaker and cross-speaker links is what CUTOFF must
        // sit inside.
        let seeds: Vec<&SegEmb> = segs.iter().filter(|s| s.dur_ms >= 2_500).collect();
        let mut sims: Vec<f32> = Vec::new();
        for a in 0..seeds.len() {
            for b in (a + 1)..seeds.len() {
                sims.push(cosine(&seeds[a].emb, &seeds[b].emb));
            }
        }
        sims.sort_by(|x, y| x.partial_cmp(y).unwrap());
        if !sims.is_empty() {
            println!(
                "pairwise sims over {} seeds: min {:.2}  p25 {:.2}  median {:.2}  p75 {:.2}  max {:.2}",
                seeds.len(),
                sims[0],
                sims[sims.len() / 4],
                sims[sims.len() / 2],
                sims[3 * sims.len() / 4],
                sims[sims.len() - 1],
            );
        }
        for cutoff in [0.45f32, 0.50, 0.55, 0.60, 0.65, 0.70, 0.75] {
            let c = cluster_at(&segs, cutoff);
            let mut sizes: Vec<usize> = c.iter().map(|x| x.seg_ids.len()).collect();
            sizes.sort_unstable_by(|a, b| b.cmp(a));
            println!("cutoff {cutoff:.2} → {} clusters {:?}", c.len(), sizes);
        }
        if let Ok(expected) = std::env::var("EXPECTED_SPEAKERS") {
            let c = cluster_with_expected(&segs, expected.parse().ok());
            let mut sizes: Vec<usize> = c.iter().map(|x| x.seg_ids.len()).collect();
            sizes.sort_unstable_by(|a, b| b.cmp(a));
            println!("expected {expected} → {} clusters {:?}", c.len(), sizes);
        }

        let named = assign_names(cluster(&segs));
        for s in &named {
            for id in &s.seg_ids {
                println!(
                    "{}  {}",
                    s.label.as_deref().unwrap_or("(default)"),
                    texts[id].chars().take(70).collect::<String>()
                );
            }
        }
        println!("→ {} speakers", named.len());
        assert!(named.len() >= 2, "a multi-speaker meeting should separate");
    }
}
