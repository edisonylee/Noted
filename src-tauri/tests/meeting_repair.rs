//! Offline meeting-repair harness: rebuild both channels of a meeting from
//! its retained WAVs, derive the them-channel timeline shift from surviving
//! rows, drop the user's own echo from `them` (mic-anchored voice proof +
//! text containment), drop speaker-playback echo from `me`, and cluster the
//! remaining remote voices against stored profiles. Read-only on the DB —
//! prints a report and writes plan TSVs for a separate, reviewed apply step.
//! Used to repair meetings 3 and 4 after the polluted-voiceprint incident
//! (commits 24a6115, and the data notes in the repo history).
//!
//!   SPEAKER_MODEL=… WHISPER_MODEL=… NOTED_DB=… MEETING_ID=3 \
//!   MEET_DIR=…/meetings/3 OUT=… \
//!   cargo test --test meeting_repair -- --ignored --nocapture
//!
//! Transcription results are cached as OUT/me_raw.tsv / them_raw.tsv — delete
//! them to force a re-transcribe.

use std::path::Path;
use tauri_app_lib::meeting::asr::{is_echo, Chunker, ChunkerCfg, EngineSpec, Transcriber, is_junk};
use tauri_app_lib::meeting::diarize::{blob_to_emb, cluster, Embedder, SegEmb};

fn read_wav(path: &str) -> Vec<f32> {
    let mut r = hound::WavReader::open(path).expect("wav");
    r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na * nb) }
}

fn rms(w: &[f32]) -> f32 {
    if w.is_empty() { return 0.0; }
    (w.iter().map(|s| s * s).sum::<f32>() / w.len() as f32).sqrt()
}

#[derive(Clone)]
struct Row {
    t0: i64,
    t1: i64,
    text: String,
}

fn transcribe_wav(wav: &[f32], whisper: &str, cache: &str) -> Vec<Row> {
    if let Ok(text) = std::fs::read_to_string(cache) {
        return text
            .lines()
            .map(|l| {
                let mut f = l.splitn(3, '\t');
                Row {
                    t0: f.next().unwrap().parse().unwrap(),
                    t1: f.next().unwrap().parse().unwrap(),
                    text: f.next().unwrap_or("").to_string(),
                }
            })
            .collect();
    }
    let mut t = Transcriber::new(&EngineSpec::Whisper { model: whisper.into() }, None)
        .expect("whisper");
    let mut chunker = Chunker::new(ChunkerCfg::default());
    let mut segs = Vec::new();
    for block in wav.chunks(16_000 * 10) {
        segs.extend(chunker.push(block));
    }
    segs.extend(chunker.flush());
    let mut rows = Vec::new();
    let mut out = String::new();
    for s in &segs {
        let Ok(text) = t.transcribe(&s.samples) else { continue };
        if is_junk(&text) {
            continue;
        }
        let text = text.replace(['\t', '\n'], " ");
        out.push_str(&format!("{}\t{}\t{}\n", s.t0_ms, s.t1_ms, text));
        rows.push(Row { t0: s.t0_ms, t1: s.t1_ms, text });
    }
    std::fs::write(cache, out).expect("cache write");
    rows
}

/// Concatenated text of rows overlapping [t0, t1] ± slack.
fn overlap_text(rows: &[Row], t0: i64, t1: i64, slack: i64) -> String {
    rows.iter()
        .filter(|r| r.t1 >= t0 - slack && r.t0 <= t1 + slack)
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
#[ignore]
fn plan() {
    let speaker_model = std::env::var("SPEAKER_MODEL").expect("SPEAKER_MODEL");
    let whisper = std::env::var("WHISPER_MODEL").expect("WHISPER_MODEL");
    let db = std::env::var("NOTED_DB").expect("NOTED_DB");
    let dir = std::env::var("MEET_DIR").expect("MEET_DIR");
    let out = std::env::var("OUT").expect("OUT");
    let meeting_id: i64 = std::env::var("MEETING_ID").expect("MEETING_ID").parse().unwrap();

    let me_wav = read_wav(&format!("{dir}/me.wav"));
    let them_wav = read_wav(&format!("{dir}/them.wav"));
    println!(
        "wavs: me {}s, them {}s",
        me_wav.len() / 16_000,
        them_wav.len() / 16_000
    );

    let me_rows = transcribe_wav(&me_wav, &whisper, &format!("{out}/me_raw.tsv"));
    let them_raw = transcribe_wav(&them_wav, &whisper, &format!("{out}/them_raw.tsv"));
    println!("rebuilt: me {} rows, them {} rows (wav time)", me_rows.len(), them_raw.len());

    // ---- shift: them.wav starts late (tap gap, wav unpadded). Anchor on the
    // surviving DB them-rows: for each, find a rebuilt row whose text contains
    // it and take the median t0 delta.
    let conn = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("db");
    let mut stmt = conn
        .prepare(
            "SELECT t0_ms, text FROM meeting_segments
             WHERE meeting_id = ?1 AND channel = 'them' ORDER BY t0_ms",
        )
        .unwrap();
    let anchors: Vec<(i64, String)> = stmt
        .query_map([meeting_id], |r| Ok((r.get(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let mut deltas = Vec::new();
    for (t0, text) in &anchors {
        let toks: Vec<String> = text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2)
            .map(|w| w.to_lowercase())
            .collect();
        if toks.len() < 3 {
            continue;
        }
        for r in &them_raw {
            let lower = r.text.to_lowercase();
            if toks.iter().all(|t| lower.contains(t.as_str())) {
                let d = t0 - r.t0;
                if (0..200_000).contains(&d) {
                    deltas.push(d);
                }
                break;
            }
        }
    }
    deltas.sort();
    assert!(!deltas.is_empty(), "no text anchors matched — cannot derive shift");
    let shift = deltas[deltas.len() / 2];
    println!("them shift: {}ms from {} anchors (spread {:?}..{:?})",
        shift, deltas.len(), deltas.first(), deltas.last());
    let them_rows: Vec<Row> = them_raw
        .iter()
        .map(|r| Row { t0: r.t0 + shift, t1: r.t1 + shift, text: r.text.clone() })
        .collect();

    // ---- profiles
    let profile = |name: &str| -> Option<Vec<f32>> {
        conn.query_row(
            "SELECT embedding FROM speaker_profiles WHERE name = ?1",
            [name],
            |r| r.get::<_, Vec<u8>>(0),
        )
        .map(|b| blob_to_emb(&b))
        .ok()
    };
    let me_print = profile("__me__").expect("__me__ profile");
    let brian_print = profile("Brian");

    let mut embedder = Embedder::new(Path::new(&speaker_model)).expect("model");

    // ---- them classification: a them-row is the user's own echo when the
    // mic at the same (shifted) instant is the user speaking live (voice
    // proof, mic-channel print — no codec mismatch) AND the row's text is
    // contained in the overlapping mic text (echo evidence).
    let mut kept: Vec<Row> = Vec::new();
    let mut kept_embs: Vec<Option<Vec<f32>>> = Vec::new();
    let (mut own_n, mut own_ms) = (0usize, 0i64);
    for r in &them_rows {
        let s0 = ((r.t0.max(0) as usize) * 16).min(me_wav.len());
        let s1 = ((r.t1.max(0) as usize) * 16).min(me_wav.len());
        let mic_span = &me_wav[s0..s1];
        let mic_is_user = rms(mic_span) >= 0.012
            && embedder
                .embed(mic_span)
                .map_or(false, |e| cosine(&e, &me_print) >= 0.60);
        let contained = is_echo(&r.text, &overlap_text(&me_rows, r.t0, r.t1, 2_500));
        if mic_is_user && contained {
            own_n += 1;
            own_ms += r.t1 - r.t0;
            continue;
        }
        // embed from them.wav (wav time!) for clustering
        let w0 = (((r.t0 - shift).max(0) as usize) * 16).min(them_wav.len());
        let w1 = (((r.t1 - shift).max(0) as usize) * 16).min(them_wav.len());
        kept_embs.push(embedder.embed(&them_wav[w0..w1]));
        kept.push(r.clone());
    }
    println!("them: {} kept, {} dropped as user's echo ({}s)", kept.len(), own_n, own_ms / 1000);

    // ---- cluster the kept them rows into speakers.
    let segs: Vec<SegEmb> = kept_embs
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            e.clone().map(|emb| SegEmb {
                seg_id: i as i64,
                dur_ms: kept[i].t1 - kept[i].t0,
                emb,
            })
        })
        .collect();
    let clusters = cluster(&segs);
    let mut label_of: Vec<Option<String>> = vec![None; kept.len()];
    let mut speaker_rows: Vec<(String, Vec<f32>, i64)> = Vec::new();
    let mut n_generic = 0usize;
    for c in &clusters {
        let brian_sim = brian_print.as_ref().map_or(0.0, |p| cosine(&c.centroid, p));
        let label = if brian_sim >= 0.55 {
            "Brian".to_string()
        } else {
            n_generic += 1;
            format!("Speaker {n_generic}")
        };
        let ms: i64 = c
            .seg_ids
            .iter()
            .map(|&i| kept[i as usize].t1 - kept[i as usize].t0)
            .sum();
        println!(
            "cluster {label}: {} segs {}s (brian_sim {brian_sim:.2}); sample: {}",
            c.seg_ids.len(),
            ms / 1000,
            c.seg_ids
                .first()
                .map(|&i| kept[i as usize].text.chars().take(60).collect::<String>())
                .unwrap_or_default()
        );
        for &i in &c.seg_ids {
            label_of[i as usize] = Some(label.clone());
        }
        speaker_rows.push((label, c.centroid.clone(), c.seg_ids.len() as i64));
    }

    // ---- me classification: rebuilt mic rows whose text is contained in the
    // kept (foreign) them text at the same instant are speaker playback.
    let mut me_keep: Vec<Row> = Vec::new();
    let (mut drop_n, mut drop_ms) = (0usize, 0i64);
    for r in &me_rows {
        if is_echo(&r.text, &overlap_text(&kept, r.t0, r.t1, 2_500)) {
            drop_n += 1;
            drop_ms += r.t1 - r.t0;
            continue;
        }
        me_keep.push(r.clone());
    }
    let me_ms: i64 = me_keep.iter().map(|r| r.t1 - r.t0).sum();
    let them_ms: i64 = kept.iter().map(|r| r.t1 - r.t0).sum();
    println!(
        "me: {} kept ({}s), {} dropped as playback ({}s)\nprojected split: me {}%",
        me_keep.len(),
        me_ms / 1000,
        drop_n,
        drop_ms / 1000,
        100 * me_ms / (me_ms + them_ms).max(1)
    );

    // ---- plan files
    let mut plan = String::new();
    for r in &me_keep {
        plan.push_str(&format!("me\t\t{}\t{}\t{}\n", r.t0, r.t1, r.text));
    }
    for (i, r) in kept.iter().enumerate() {
        plan.push_str(&format!(
            "them\t{}\t{}\t{}\t{}\n",
            label_of[i].as_deref().unwrap_or(""),
            r.t0,
            r.t1,
            r.text
        ));
    }
    std::fs::write(format!("{out}/plan.tsv"), plan).unwrap();
    let mut spk = String::new();
    for (label, centroid, n) in &speaker_rows {
        spk.push_str(&format!(
            "{label}\t{n}\t{}\n",
            centroid.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",")
        ));
    }
    std::fs::write(format!("{out}/speakers.tsv"), spk).unwrap();
    println!("plan written to {out}/plan.tsv");
}
