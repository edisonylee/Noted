//! Offline room-mic diarization through the bundled FluidAudio Swift helper.
//! FluidAudio owns model download/Core ML execution; this module keeps the
//! helper boundary small and maps its time ranges onto Noted's ASR segments.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use tauri::Manager;

use crate::db::Db;

use super::store;

const MODEL_FILES: &[&str] = &[
    "Segmentation.mlmodelc",
    "FBank.mlmodelc",
    "Embedding.mlmodelc",
    "PldaRho.mlmodelc",
    "plda-parameters.json",
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FluidSegment {
    speaker_id: String,
    start_ms: i64,
    end_ms: i64,
}

#[derive(Debug, Deserialize)]
struct FluidResult {
    segments: Vec<FluidSegment>,
    #[serde(default)]
    speakers: HashMap<String, Vec<f32>>,
}

pub fn supported() -> bool {
    #[cfg(target_os = "macos")]
    {
        let version = Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .unwrap_or_default();
        version
            .trim()
            .split('.')
            .next()
            .and_then(|major| major.parse::<u32>().ok())
            .is_some_and(|major| major >= 14)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn helper_path(app: &tauri::AppHandle) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("NOTED_FLUID_DIARIZER_PATH") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Ok(resource_dir) = app.path().resource_dir() {
        for path in [
            resource_dir.join("noted-fluid-diarizer"),
            resource_dir.join("resources").join("noted-fluid-diarizer"),
        ] {
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    let development = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("noted-fluid-diarizer");
    if development.is_file() {
        return Ok(development);
    }
    Err(anyhow!(
        "the in-person speaker helper is missing; rebuild the desktop app"
    ))
}

pub fn models_dir(app: &tauri::AppHandle) -> Result<PathBuf> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|error| anyhow!("{error}"))?
        .join("models")
        .join("fluid-audio"))
}

pub fn ready(app: &tauri::AppHandle) -> bool {
    if !supported() || helper_path(app).is_err() {
        return false;
    }
    let Ok(directory) = models_dir(app) else {
        return false;
    };
    let repo = directory.join("speaker-diarization");
    MODEL_FILES.iter().all(|name| repo.join(name).exists())
}

pub fn prepare(app: &tauri::AppHandle) -> Result<()> {
    if !supported() {
        return Err(anyhow!(
            "in-person speaker separation requires macOS 14 or newer"
        ));
    }
    let helper = helper_path(app)?;
    let models = models_dir(app)?;
    std::fs::create_dir_all(&models)?;
    let output = Command::new(helper)
        .arg("prepare")
        .arg("--models-dir")
        .arg(&models)
        .output()
        .context("could not start the FluidAudio model setup")?;
    if !output.status.success() {
        return Err(helper_error(
            "FluidAudio model setup failed",
            &output.stderr,
        ));
    }
    Ok(())
}

pub fn diarize_meeting(
    app: &tauri::AppHandle,
    meeting_id: i64,
    audio_path: &Path,
) -> Result<usize> {
    if !ready(app) {
        return Err(anyhow!(
            "in-person speaker separation is not ready; download it in Settings → Meetings"
        ));
    }
    let output = Command::new(helper_path(app)?)
        .arg("diarize")
        .arg("--models-dir")
        .arg(models_dir(app)?)
        .arg("--audio")
        .arg(audio_path)
        .output()
        .context("could not start FluidAudio speaker separation")?;
    if !output.status.success() {
        return Err(helper_error(
            "FluidAudio speaker separation failed",
            &output.stderr,
        ));
    }
    let result: FluidResult = serde_json::from_slice(&output.stdout)
        .context("FluidAudio returned an unreadable result")?;

    let db = app.state::<Db>();
    let conn = db.0.lock().unwrap();
    let transcript = store::segment_times(&conn, meeting_id, "me")?;
    let assignment = assign_speakers(&transcript, &result.segments);

    store::clear_channel_speakers(&conn, meeting_id, "me")?;
    store::clear_meeting_speakers(&conn, meeting_id)?;
    store::set_segment_speakers(&conn, &assignment.labels)?;

    let rows = assignment
        .speaker_ids
        .iter()
        .map(|(raw_id, label)| {
            (
                label.clone(),
                result.speakers.get(raw_id).cloned().unwrap_or_default(),
                assignment.counts.get(raw_id).copied().unwrap_or(0),
            )
        })
        .filter(|(_, _, count)| *count > 0)
        .collect::<Vec<_>>();
    store::save_meeting_speakers(&conn, meeting_id, &rows)?;
    Ok(rows.len())
}

fn helper_error(prefix: &str, stderr: &[u8]) -> anyhow::Error {
    let detail = String::from_utf8_lossy(stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        anyhow!(prefix.to_string())
    } else {
        anyhow!("{prefix}: {detail}")
    }
}

#[derive(Debug, Default)]
struct Assignment {
    labels: Vec<(i64, String)>,
    speaker_ids: BTreeMap<String, String>,
    counts: HashMap<String, i64>,
}

fn assign_speakers(transcript: &[(i64, i64, i64)], fluid: &[FluidSegment]) -> Assignment {
    let mut first_seen = HashMap::<String, i64>::new();
    for segment in fluid {
        first_seen
            .entry(segment.speaker_id.clone())
            .and_modify(|start| *start = (*start).min(segment.start_ms))
            .or_insert(segment.start_ms);
    }
    let mut ordered = first_seen.into_iter().collect::<Vec<_>>();
    ordered.sort_by_key(|(speaker, start)| (*start, speaker.clone()));
    let speaker_ids = ordered
        .into_iter()
        .enumerate()
        .map(|(index, (speaker, _))| (speaker, format!("Speaker {}", index + 1)))
        .collect::<BTreeMap<_, _>>();

    let mut labels = Vec::new();
    let mut counts = HashMap::<String, i64>::new();
    for &(segment_id, start, end) in transcript {
        let mut overlaps = HashMap::<&str, i64>::new();
        for diarized in fluid {
            let overlap = end.min(diarized.end_ms) - start.max(diarized.start_ms);
            if overlap > 0 {
                *overlaps.entry(&diarized.speaker_id).or_default() += overlap;
            }
        }
        let selected = overlaps
            .into_iter()
            .max_by(|(a_id, a_overlap), (b_id, b_overlap)| {
                a_overlap.cmp(b_overlap).then_with(|| b_id.cmp(a_id))
            })
            .map(|(speaker, _)| speaker);
        if let Some(raw_id) = selected {
            if let Some(label) = speaker_ids.get(raw_id) {
                labels.push((segment_id, label.clone()));
                *counts.entry(raw_id.to_string()).or_default() += 1;
            }
        }
    }
    Assignment {
        labels,
        speaker_ids,
        counts,
    }
}

#[cfg(test)]
mod tests {
    use super::{assign_speakers, FluidSegment};

    fn segment(speaker: &str, start_ms: i64, end_ms: i64) -> FluidSegment {
        FluidSegment {
            speaker_id: speaker.into(),
            start_ms,
            end_ms,
        }
    }

    #[test]
    fn labels_follow_first_speaker_appearance_and_maximum_overlap() {
        let transcript = vec![(10, 0, 1_000), (11, 1_000, 2_000)];
        let diarized = vec![
            segment("B", 0, 800),
            segment("A", 800, 1_400),
            segment("A", 1_400, 2_000),
        ];
        let result = assign_speakers(&transcript, &diarized);
        assert_eq!(
            result.labels,
            vec![(10, "Speaker 1".into()), (11, "Speaker 2".into())]
        );
        assert_eq!(result.counts["B"], 1);
        assert_eq!(result.counts["A"], 1);
    }

    #[test]
    fn leaves_silence_only_transcript_lines_unassigned() {
        let result = assign_speakers(&[(10, 0, 500)], &[segment("A", 600, 900)]);
        assert!(result.labels.is_empty());
    }
}
