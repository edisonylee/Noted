// Deterministic conversation mechanics derived from the persisted transcript.
// These are observations, not judgments: no sentiment, enthusiasm, attention,
// or composite meeting score is inferred here.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

const TURN_GAP_MS: i64 = 1_500;
const MIN_ANALYTICS_MS: i64 = 15_000;
const MIN_PACE_MS: i64 = 60_000;
const MIN_PACE_WORDS: usize = 100;
const MIN_SPEAKER_COVERAGE_PCT: i64 = 90;

#[derive(Debug)]
struct SpeakerStats {
    label: String,
    channel: &'static str,
    first_seen: usize,
    talk_ms: i64,
    words: usize,
    last_end_ms: Option<i64>,
    speech_bursts_ms: Vec<i64>,
}

fn word_count(text: &str) -> usize {
    text.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .count()
}

fn segment_span(segment: &Value) -> i64 {
    (segment["t1_ms"].as_i64().unwrap_or(0) - segment["t0_ms"].as_i64().unwrap_or(0)).max(0)
}

fn has_voice_timing(segment: &Value) -> bool {
    segment
        .get("voiced_ms")
        .and_then(Value::as_i64)
        .is_some_and(|voiced| voiced >= 0 && voiced <= segment_span(segment))
}

fn segment_timing(segment: &Value, use_voice_timing: bool) -> i64 {
    let span =
        (segment["t1_ms"].as_i64().unwrap_or(0) - segment["t0_ms"].as_i64().unwrap_or(0)).max(0);
    if use_voice_timing {
        segment["voiced_ms"].as_i64().unwrap_or(span)
    } else {
        span
    }
}

fn remote_label(segment: &Value) -> Option<&str> {
    segment
        .get("speaker")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|label| !label.is_empty() && !label.eq_ignore_ascii_case("them"))
}

fn collect_stats(segments: &[Value], detailed: bool, use_voice_timing: bool) -> Vec<Value> {
    let mut ordered: Vec<&Value> = segments.iter().collect();
    ordered.sort_by_key(|segment| {
        (
            segment["t0_ms"].as_i64().unwrap_or(0),
            segment["id"].as_i64().unwrap_or(0),
        )
    });

    let mut by_label: HashMap<(String, String), usize> = HashMap::new();
    let mut stats: Vec<SpeakerStats> = Vec::new();

    for (position, segment) in ordered.into_iter().enumerate() {
        let channel = segment["channel"].as_str().unwrap_or("them");
        let (label, channel_name) = if channel == "me" {
            ("You".to_string(), "me")
        } else if detailed {
            (
                remote_label(segment).unwrap_or("Unassigned").to_string(),
                "them",
            )
        } else {
            ("Others".to_string(), "them")
        };
        let label = if channel_name == "them" && label.eq_ignore_ascii_case("you") {
            "You (remote)".to_string()
        } else {
            label
        };
        let key = (channel_name.to_string(), label.clone());
        let index = *by_label.entry(key).or_insert_with(|| {
            stats.push(SpeakerStats {
                label: label.clone(),
                channel: channel_name,
                first_seen: position,
                talk_ms: 0,
                words: 0,
                last_end_ms: None,
                speech_bursts_ms: Vec::new(),
            });
            stats.len() - 1
        });

        let talk_ms = segment_timing(segment, use_voice_timing);
        stats[index].talk_ms += talk_ms;
        stats[index].words += word_count(segment["text"].as_str().unwrap_or(""));

        let t0 = segment["t0_ms"].as_i64().unwrap_or(0);
        let t1 = segment["t1_ms"].as_i64().unwrap_or(t0);
        let continues = stats[index]
            .last_end_ms
            .is_some_and(|previous_end| t0.saturating_sub(previous_end) <= TURN_GAP_MS);
        if continues {
            if let Some(burst) = stats[index].speech_bursts_ms.last_mut() {
                *burst += talk_ms;
            }
        } else {
            stats[index].speech_bursts_ms.push(talk_ms);
        }
        stats[index].last_end_ms = Some(stats[index].last_end_ms.map_or(t1, |end| end.max(t1)));
    }

    stats.sort_by_key(|speaker| speaker.first_seen);
    let total_ms = stats.iter().map(|speaker| speaker.talk_ms).sum::<i64>();
    stats
        .into_iter()
        .map(|mut speaker| {
            speaker.speech_bursts_ms.sort_unstable();
            let median_speech_burst_ms = match speaker.speech_bursts_ms.len() {
                0 => 0,
                len if len % 2 == 1 => speaker.speech_bursts_ms[len / 2],
                len => {
                    (speaker.speech_bursts_ms[len / 2 - 1] + speaker.speech_bursts_ms[len / 2]) / 2
                }
            };
            let pace_wpm = (use_voice_timing
                && speaker.talk_ms >= MIN_PACE_MS
                && speaker.words >= MIN_PACE_WORDS)
                .then(|| {
                    ((speaker.words as f64 * 60_000.0) / speaker.talk_ms as f64).round() as i64
                });
            let share_pct = if total_ms > 0 {
                (speaker.talk_ms as f64 * 1000.0 / total_ms as f64).round() / 10.0
            } else {
                0.0
            };
            json!({
                "label": speaker.label,
                "channel": speaker.channel,
                "talk_ms": speaker.talk_ms,
                "share_pct": share_pct,
                "words": speaker.words,
                "pace_wpm": pace_wpm,
                "speech_bursts": speaker.speech_bursts_ms.len(),
                "median_speech_burst_ms": median_speech_burst_ms,
            })
        })
        .collect()
}

/// Build the meeting-detail analytics payload. Speaker detail is withheld when
/// calendar headcount and diarization disagree or too much remote speech is
/// unassigned; the deterministic mic/system channel view remains available.
pub fn build(segments: &[Value], expected_remote_speakers: Option<usize>) -> Value {
    // Mixing padded legacy spans with speech-only timing would bias shares.
    // Use one timing basis consistently for the whole meeting.
    let use_voice_timing = !segments.is_empty() && segments.iter().all(has_voice_timing);
    let mut remote_ms = 0i64;
    let mut labeled_remote_ms = 0i64;
    let mut detected = HashSet::new();
    let mut total_ms = 0i64;
    let mut total_words = 0usize;

    for segment in segments {
        let talk_ms = segment_timing(segment, use_voice_timing);
        total_ms += talk_ms;
        total_words += word_count(segment["text"].as_str().unwrap_or(""));
        if segment["channel"].as_str().unwrap_or("them") == "them" {
            remote_ms += talk_ms;
            if let Some(label) = remote_label(segment) {
                labeled_remote_ms += talk_ms;
                detected.insert(label.to_lowercase());
            }
        }
    }

    let coverage_pct = if remote_ms > 0 {
        Some((labeled_remote_ms * 100 / remote_ms).clamp(0, 100))
    } else {
        None
    };
    let unattributed_remote_ms = (remote_ms - labeled_remote_ms).max(0);
    let unattributed_remote_pct = (remote_ms > 0)
        .then(|| (unattributed_remote_ms as f64 * 1_000.0 / remote_ms as f64).round() / 10.0);
    let count_matches = expected_remote_speakers.is_none_or(|count| count == detected.len());
    let enough_speech = total_ms >= MIN_ANALYTICS_MS;
    let speaker_detail_available = enough_speech
        && remote_ms > 0
        && coverage_pct.is_some_and(|coverage| coverage >= MIN_SPEAKER_COVERAGE_PCT)
        && count_matches;
    let speaker_detail_reason = if !enough_speech {
        "not_enough_speech"
    } else if remote_ms == 0 {
        "no_remote_speech"
    } else if coverage_pct.is_none_or(|coverage| coverage < MIN_SPEAKER_COVERAGE_PCT) {
        "low_attribution"
    } else if !count_matches {
        "speaker_count_mismatch"
    } else {
        "available"
    };

    json!({
        "available": enough_speech,
        "timing_basis": if use_voice_timing { "voice_activity" } else { "segment_bounds" },
        "speaker_time_ms": total_ms,
        "transcript_words": total_words,
        "expected_remote_speakers": expected_remote_speakers,
        "detected_remote_speakers": detected.len(),
        "speaker_coverage_pct": coverage_pct,
        "unattributed_remote_ms": unattributed_remote_ms,
        "unattributed_remote_pct": unattributed_remote_pct,
        "speaker_detail_available": speaker_detail_available,
        "speaker_detail_reason": speaker_detail_reason,
        "channels": collect_stats(segments, false, use_voice_timing),
        "speakers": if speaker_detail_available {
            collect_stats(segments, true, use_voice_timing)
                .into_iter()
                .filter(|speaker| {
                    speaker["channel"] != "them" || speaker["label"] != "Unassigned"
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(count: usize) -> String {
        (0..count).map(|_| "word").collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn historical_group_meeting_stays_at_channel_level_when_count_mismatches() {
        let segments = vec![
            json!({ "id": 1, "channel": "me", "t0_ms": 0, "t1_ms": 10_000, "text": "hello there", "speaker": null }),
            json!({ "id": 2, "channel": "them", "t0_ms": 10_000, "t1_ms": 30_000, "text": "team update", "speaker": "Speaker 2" }),
        ];
        let out = build(&segments, Some(4));
        assert_eq!(out["speaker_detail_available"], false);
        assert_eq!(out["speaker_detail_reason"], "speaker_count_mismatch");
        assert_eq!(out["timing_basis"], "segment_bounds");
        assert_eq!(out["channels"].as_array().unwrap().len(), 2);
        assert!(out["speakers"].as_array().unwrap().is_empty());
        assert!(out["channels"].as_array().unwrap()[0]["pace_wpm"].is_null());
    }

    #[test]
    fn complete_diarization_unlocks_speaker_detail() {
        let segments = vec![
            json!({ "id": 1, "channel": "me", "t0_ms": 0, "t1_ms": 10_000, "voiced_ms": 8_000, "text": "start", "speaker": null }),
            json!({ "id": 2, "channel": "them", "t0_ms": 10_000, "t1_ms": 20_000, "voiced_ms": 8_000, "text": "alpha", "speaker": "Ana" }),
            json!({ "id": 3, "channel": "them", "t0_ms": 20_000, "t1_ms": 30_000, "voiced_ms": 8_000, "text": "beta", "speaker": "Bo" }),
        ];
        let out = build(&segments, Some(2));
        assert_eq!(out["speaker_detail_available"], true);
        assert_eq!(out["speaker_coverage_pct"], 100);
        assert_eq!(out["speakers"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn unattributed_audio_is_not_counted_as_an_estimated_speaker() {
        let segments = vec![
            json!({ "id": 1, "channel": "me", "t0_ms": 0, "t1_ms": 10_000, "voiced_ms": 8_000, "text": "start", "speaker": null }),
            json!({ "id": 2, "channel": "them", "t0_ms": 10_000, "t1_ms": 14_000, "voiced_ms": 4_000, "text": "one", "speaker": "Speaker 1" }),
            json!({ "id": 3, "channel": "them", "t0_ms": 14_000, "t1_ms": 18_000, "voiced_ms": 4_000, "text": "two", "speaker": "Speaker 2" }),
            json!({ "id": 4, "channel": "them", "t0_ms": 18_000, "t1_ms": 22_000, "voiced_ms": 4_000, "text": "three", "speaker": "Speaker 3" }),
            json!({ "id": 5, "channel": "them", "t0_ms": 22_000, "t1_ms": 26_000, "voiced_ms": 4_000, "text": "four", "speaker": "Speaker 4" }),
            json!({ "id": 6, "channel": "them", "t0_ms": 26_000, "t1_ms": 27_000, "voiced_ms": 1_000, "text": "unclear", "speaker": null }),
        ];

        let out = build(&segments, Some(4));
        assert_eq!(out["speaker_detail_available"], true);
        assert_eq!(out["detected_remote_speakers"], 4);
        assert_eq!(out["speakers"].as_array().unwrap().len(), 5);
        assert!(!out["speakers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|speaker| speaker["label"] == "Unassigned"));
        assert_eq!(out["unattributed_remote_ms"], 1_000);
        assert_eq!(out["unattributed_remote_pct"], 5.9);
    }

    #[test]
    fn pace_requires_voice_timing_and_a_real_sample() {
        let transcript = words(180);
        let segments = vec![json!({
            "id": 1, "channel": "me", "t0_ms": 0, "t1_ms": 70_000,
            "voiced_ms": 60_000, "text": transcript, "speaker": null
        })];
        let out = build(&segments, None);
        assert_eq!(out["channels"][0]["pace_wpm"], 180);
        assert_eq!(out["timing_basis"], "voice_activity");
    }

    #[test]
    fn forced_asr_splits_do_not_create_fake_turns() {
        let segments = vec![
            json!({ "id": 1, "channel": "me", "t0_ms": 0, "t1_ms": 12_000, "voiced_ms": 10_000, "text": "one", "speaker": null }),
            json!({ "id": 2, "channel": "me", "t0_ms": 12_000, "t1_ms": 24_000, "voiced_ms": 10_000, "text": "two", "speaker": null }),
            json!({ "id": 3, "channel": "them", "t0_ms": 24_000, "t1_ms": 30_000, "voiced_ms": 5_000, "text": "reply", "speaker": "Ana" }),
        ];
        let out = build(&segments, Some(1));
        assert_eq!(out["channels"][0]["speech_bursts"], 1);
        assert_eq!(out["channels"][1]["speech_bursts"], 1);
    }

    #[test]
    fn mixed_rows_use_one_conservative_timing_basis() {
        let segments = vec![
            json!({ "id": 1, "channel": "me", "t0_ms": 0, "t1_ms": 10_000, "voiced_ms": 2_000, "text": "one", "speaker": null }),
            json!({ "id": 2, "channel": "them", "t0_ms": 10_000, "t1_ms": 30_000, "text": "two", "speaker": "Ana" }),
        ];
        let out = build(&segments, Some(1));
        assert_eq!(out["timing_basis"], "segment_bounds");
        assert_eq!(out["channels"][0]["talk_ms"], 10_000);
        assert_eq!(out["channels"][1]["talk_ms"], 20_000);
    }

    #[test]
    fn no_remote_speech_has_no_fake_coverage() {
        let segments = vec![json!({
            "id": 1, "channel": "me", "t0_ms": 0, "t1_ms": 20_000,
            "voiced_ms": 18_000, "text": "solo", "speaker": null
        })];
        let out = build(&segments, Some(0));
        assert!(out["speaker_coverage_pct"].is_null());
        assert_eq!(out["speaker_detail_reason"], "no_remote_speech");
    }
}
