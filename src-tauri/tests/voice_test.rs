// Runtime validation for whisper.cpp transcription, using the known JFK sample
// (16 kHz mono). Requires /tmp/ggml-base.en.bin and /tmp/jfk.wav (fetched by the
// harness). Skips cleanly if either is absent.
use std::path::Path;
use tauri_app_lib::{meeting, voice};

#[test]
fn transcribes_jfk_sample() {
    let model = Path::new("/tmp/ggml-base.en.bin");
    let wav = Path::new("/tmp/jfk.wav");
    if !model.exists() || !wav.exists() {
        eprintln!("skip: model or wav not present");
        return;
    }
    let mut reader = hound::WavReader::open(wav).unwrap();
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect();

    let text = voice::transcribe(model, &samples, None).unwrap();
    println!("--- transcript ---\n{text}");
    let lc = text.to_lowercase();
    assert!(
        lc.contains("country") || lc.contains("americans"),
        "expected JFK words, got: {text}"
    );
}

// Mirrors exactly what the `transcribe` command does with what the UI sends:
// f32 samples -> little-endian bytes -> base64 -> decode -> resample -> transcribe.
#[test]
fn base64_f32_roundtrip_transcribes() {
    use base64::Engine;
    let model = Path::new("/tmp/ggml-base.en.bin");
    let wav = Path::new("/tmp/jfk.wav");
    if !model.exists() || !wav.exists() {
        eprintln!("skip");
        return;
    }
    let mut reader = hound::WavReader::open(wav).unwrap();
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect();

    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for s in &samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .unwrap();
    let back: Vec<f32> = decoded
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let resampled = voice::resample_to_16k(&back, 16_000);

    let text = voice::transcribe(model, &resampled, None).unwrap();
    assert!(text.to_lowercase().contains("country"), "got: {text}");
}

// Manual recovery/diagnostic harness for retained meeting WAVs. It is ignored
// in the normal suite because it loads the full local model and accepts a
// user-selected audio slice through environment variables.
#[test]
#[ignore]
fn transcribes_retained_meeting_slice() {
    let model = std::env::var("NOTED_WHISPER_MODEL").expect("NOTED_WHISPER_MODEL");
    let wav = std::env::var("NOTED_TEST_WAV").expect("NOTED_TEST_WAV");
    let start_secs: usize = std::env::var("NOTED_TEST_START")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let duration_secs: usize = std::env::var("NOTED_TEST_DURATION")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let mut reader = hound::WavReader::open(wav).unwrap();
    assert_eq!(reader.spec().sample_rate, 16_000);
    let start = start_secs * 16_000;
    let take = duration_secs * 16_000;
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .skip(start)
        .take(take)
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect();
    let text = voice::transcribe(Path::new(&model), &samples, None).unwrap();
    println!(
        "--- {start_secs}s..{}s ---\n{text}",
        start_secs + duration_secs
    );
}

#[test]
#[ignore]
fn retranscribes_retained_meeting_to_json() {
    let model = std::env::var("NOTED_WHISPER_MODEL").expect("NOTED_WHISPER_MODEL");
    let wav = std::env::var("NOTED_TEST_WAV").expect("NOTED_TEST_WAV");
    let output = std::env::var("NOTED_TEST_OUTPUT").expect("NOTED_TEST_OUTPUT");
    let segments = meeting::asr::transcribe_retained_wav(
        &meeting::asr::EngineSpec::Whisper {
            model: model.into(),
        },
        Path::new(&wav),
        None,
        &[],
    )
    .unwrap();
    std::fs::write(&output, serde_json::to_vec_pretty(&segments).unwrap()).unwrap();
    println!("wrote {} recovered segments to {output}", segments.len());
}
