// Local speech-to-text via whisper.cpp (whisper-rs). Fully offline + free.
// Expects 16 kHz mono f32 PCM (the frontend resamples before sending).

use std::path::Path;

use anyhow::{anyhow, Result};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// whisper.cpp needs 16 kHz mono. Linear-resample if the source rate differs.
pub fn resample_to_16k(samples: &[f32], from_rate: u32) -> Vec<f32> {
    if from_rate == 16_000 || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = 16_000.0 / from_rate as f32;
    let out_len = (samples.len() as f32 * ratio) as usize;
    (0..out_len)
        .map(|i| {
            let src = i as f32 / ratio;
            let j = src as usize;
            let frac = src - j as f32;
            let a = samples[j.min(samples.len() - 1)];
            let b = samples[(j + 1).min(samples.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

pub fn transcribe(model_path: &Path, samples: &[f32], hint: Option<&str>) -> Result<String> {
    let path = model_path
        .to_str()
        .ok_or_else(|| anyhow!("bad model path"))?;
    let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
        .map_err(|e| anyhow!("whisper load failed: {e:?}"))?;
    let mut state = ctx
        .create_state()
        .map_err(|e| anyhow!("whisper state: {e:?}"))?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("en"));
    params.set_print_progress(false);
    params.set_print_special(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    if let Some(h) = hint {
        // Decoder bias toward the user's vocabulary (set_initial_prompt
        // panics on interior NULs).
        params.set_initial_prompt(&h.replace('\0', ""));
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
