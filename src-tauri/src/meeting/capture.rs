// System-audio + mic capture for the meeting recorder.
//
// System audio: a Core Audio *process tap* (macOS 14.4+) on every process
// except noted itself, wrapped in a private aggregate device — the exact
// recipe from cidre's own core-audio-record example (and what shipped
// Granola-class recorders use). Needs the one-time "System Audio Recording"
// permission (NSAudioCaptureUsageDescription in Info.plist); a denial shows
// up as silent buffers, not an error.
//
// Mic: cpal on the default input device.
//
// cpal streams are !Send and Core Audio teardown is cleanest on the creating
// thread, so each capture runs on a dedicated thread: build → poll stop flag →
// tear down in place. Both sides push mono f32 at native rate into a shared
// ChannelBuf; the ASR worker drains and resamples to 16 kHz.
//
// Known tap failure modes (from shipped recorders) handled by session rebuild:
//   - the tap silently stops delivering callbacks (watchdog on last_callback)
//   - the default output device changes (AirPods!) — uid checked each tick

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Samples accumulated by a capture callback, drained by the ASR worker.
/// Mono f32 at `sample_rate` (native; the drainer resamples).
pub struct ChannelBuf {
    pub samples: Mutex<Vec<f32>>,
    pub sample_rate: AtomicU32,
    /// epoch ms of the last callback of any kind (watchdog: stalls → rebuild).
    pub last_callback: AtomicU64,
    /// epoch ms of the last clearly non-silent buffer (drives silence auto-stop).
    pub last_signal: AtomicU64,
}

impl ChannelBuf {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            samples: Mutex::new(Vec::new()),
            sample_rate: AtomicU32::new(0),
            last_callback: AtomicU64::new(0),
            last_signal: AtomicU64::new(0),
        })
    }

    /// Push mono samples from a callback. Caps the backlog at ~2 minutes so a
    /// stalled consumer can't grow memory unboundedly (that would mean the ASR
    /// worker died — the recording is already lost at that point).
    pub fn push(&self, mono: &[f32]) {
        let now = epoch_ms();
        self.last_callback.store(now, Ordering::Relaxed);
        if mono.iter().any(|s| s.abs() > 0.004) {
            self.last_signal.store(now, Ordering::Relaxed);
        }
        let cap = (self.sample_rate.load(Ordering::Relaxed).max(16_000) as usize) * 120;
        let mut buf = self.samples.lock().unwrap();
        if buf.len() + mono.len() > cap {
            buf.clear();
        }
        buf.extend_from_slice(mono);
    }

    /// Take everything accumulated since the last drain.
    pub fn drain(&self) -> (Vec<f32>, u32) {
        let rate = self.sample_rate.load(Ordering::Relaxed);
        let mut buf = self.samples.lock().unwrap();
        (std::mem::take(&mut *buf), rate)
    }
}

/// Downmix an interleaved buffer to mono in place-ish (averaging channels).
pub fn downmix_mono(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

// ---------------------------------------------------------------------------
// Mic capture (cpal), on its own thread.
// ---------------------------------------------------------------------------

pub fn run_mic(buf: Arc<ChannelBuf>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match mic_session(&buf, &stop) {
            Ok(()) => break, // clean stop
            Err(e) => {
                eprintln!("[noted] mic capture error (retrying in 2s): {e}");
                // Device unplugged / config change: retry until stopped.
                for _ in 0..8 {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }
}

fn mic_session(buf: &Arc<ChannelBuf>, stop: &Arc<AtomicBool>) -> Result<()> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("no default input device"))?;
    let config = device.default_input_config()?;
    let rate = config.sample_rate();
    let channels = config.channels() as usize;
    buf.sample_rate.store(rate, Ordering::Relaxed);

    let cb_buf = buf.clone();
    let err_flag = Arc::new(AtomicBool::new(false));
    let err_cb = err_flag.clone();
    let stream = device.build_input_stream(
        &config.into(),
        move |data: &[f32], _| {
            cb_buf.push(&downmix_mono(data, channels));
        },
        move |e| {
            eprintln!("[noted] mic stream error: {e}");
            err_cb.store(true, Ordering::Relaxed);
        },
        None,
    )?;
    stream.play()?;

    while !stop.load(Ordering::Relaxed) {
        if err_flag.load(Ordering::Relaxed) {
            return Err(anyhow!("mic stream errored"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// System-audio capture (Core Audio process tap), macOS only.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub fn tap_supported() -> bool {
    // Process taps exist from 14.2 but the public permission flow works from
    // 14.4 (AudioCap / Rogue Amoeba both cite it). Runtime gate, not compile.
    let ver = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let mut it = ver.trim().split('.');
    let major: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    major > 14 || (major == 14 && minor >= 4)
}

#[cfg(not(target_os = "macos"))]
pub fn tap_supported() -> bool {
    false
}

#[cfg(target_os = "macos")]
pub fn run_system_tap(buf: Arc<ChannelBuf>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match macos::tap_session(&buf, &stop) {
            Ok(()) => break, // clean stop
            Err(e) => {
                eprintln!("[noted] system tap error (rebuilding in 2s): {e}");
                for _ in 0..8 {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn run_system_tap(_buf: Arc<ChannelBuf>, _stop: Arc<AtomicBool>) {}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use cidre::core_audio::aggregate_device_keys as agg_keys;
    use cidre::core_audio::sub_device_keys as sub_keys;
    use cidre::{arc, av, cat, cf, core_audio as ca, ns, os};

    struct Ctx {
        buf: Arc<ChannelBuf>,
        format: arc::R<av::AudioFormat>,
        channels: usize,
    }

    extern "C" fn io_proc(
        _device: ca::Device,
        _now: &cat::AudioTimeStamp,
        input_data: &cat::AudioBufList<1>,
        _input_time: &cat::AudioTimeStamp,
        _output_data: &mut cat::AudioBufList<1>,
        _output_time: &cat::AudioTimeStamp,
        ctx: Option<&mut Ctx>,
    ) -> os::Status {
        let Some(ctx) = ctx else {
            return Default::default();
        };
        if let Some(view) = av::AudioPcmBuf::with_buf_list_no_copy(&ctx.format, input_data, None) {
            if let Some(data) = view.data_f32_at(0) {
                // Mono tap → channel 0 is the whole signal. (If the format ever
                // comes back interleaved multi-channel, downmix.)
                if ctx.channels <= 1 {
                    ctx.buf.push(data);
                } else {
                    ctx.buf.push(&downmix_mono(data, ctx.channels));
                }
                return Default::default();
            }
        }
        // Fallback: raw first-buffer read (Meetily's path for odd formats).
        let b = &input_data.buffers[0];
        let n = b.data_bytes_size as usize / std::mem::size_of::<f32>();
        if n > 0 && !b.data.is_null() {
            let data = unsafe { std::slice::from_raw_parts(b.data as *const f32, n) };
            ctx.buf.push(&downmix_mono(data, ctx.channels.max(1)));
        }
        Default::default()
    }

    /// One tap lifetime: build → run until stop / stall / device change.
    /// Ok(()) = clean stop; Err = rebuild wanted.
    pub fn tap_session(buf: &Arc<ChannelBuf>, stop: &Arc<AtomicBool>) -> Result<()> {
        let output_device = ca::System::default_output_device()
            .map_err(|e| anyhow!("no default output device: {e:?}"))?;
        let output_uid = output_device
            .uid()
            .map_err(|e| anyhow!("output uid: {e:?}"))?;

        // Exclude noted's own audio (e.g. notification sounds) from the tap.
        let exclude = match ca::Process::with_pid(std::process::id() as i32) {
            Ok(me) => ns::Array::from_slice(&[ns::Number::with_u32(me.0 .0).as_ref()]),
            Err(_) => ns::Array::new(),
        };
        let tap_desc = ca::TapDesc::with_mono_global_tap_excluding_processes(&exclude);
        let tap = tap_desc
            .create_process_tap()
            .map_err(|e| anyhow!("create tap failed (permission denied or unsupported): {e:?}"))?;

        let asbd = tap.asbd().map_err(|e| anyhow!("tap format: {e:?}"))?;
        buf.sample_rate
            .store(asbd.sample_rate as u32, Ordering::Relaxed);
        let channels = asbd.channels_per_frame as usize;
        let format =
            av::AudioFormat::with_asbd(&asbd).ok_or_else(|| anyhow!("bad tap format"))?;

        let sub_device =
            cf::DictionaryOf::with_keys_values(&[sub_keys::uid()], &[output_uid.as_type_ref()]);
        let tap_uid = tap.uid().map_err(|e| anyhow!("tap uid: {e:?}"))?;
        let sub_tap =
            cf::DictionaryOf::with_keys_values(&[sub_keys::uid()], &[tap_uid.as_type_ref()]);

        // The "magic" private-aggregate-device composition from cidre's
        // core-audio-record example: output device as main sub-device, the tap
        // in the tap list, auto-started, private (invisible in Sound settings).
        let dict = cf::DictionaryOf::with_keys_values(
            &[
                agg_keys::is_private(),
                agg_keys::is_stacked(),
                agg_keys::tap_auto_start(),
                agg_keys::name(),
                agg_keys::main_sub_device(),
                agg_keys::uid(),
                agg_keys::sub_device_list(),
                agg_keys::tap_list(),
            ],
            &[
                cf::Boolean::value_true().as_type_ref(),
                cf::Boolean::value_false(),
                cf::Boolean::value_true(),
                cf::str!(c"noted-meeting-tap"),
                &output_uid,
                &cf::Uuid::new().to_cf_string(),
                &cf::ArrayOf::from_slice(&[sub_device.as_ref()]),
                &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
            ],
        );
        let agg_device = ca::AggregateDevice::with_desc(&dict)
            .map_err(|e| anyhow!("aggregate device: {e:?}"))?;

        let mut ctx = Ctx {
            buf: buf.clone(),
            format,
            channels,
        };
        let proc_id = agg_device
            .create_io_proc_id(io_proc, Some(&mut ctx))
            .map_err(|e| anyhow!("io proc: {e:?}"))?;
        let started = ca::device_start(&*agg_device, Some(proc_id))
            .map_err(|e| anyhow!("device start: {e:?}"))?;

        // Poll loop: clean stop, watchdog stall, or output-device switch.
        buf.last_callback.store(epoch_ms(), Ordering::Relaxed);
        let result = loop {
            if stop.load(Ordering::Relaxed) {
                break Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));

            let stale_ms = epoch_ms().saturating_sub(buf.last_callback.load(Ordering::Relaxed));
            if stale_ms > 10_000 {
                break Err(anyhow!("tap delivered no callbacks for {stale_ms}ms"));
            }
            if let Ok(current) = ca::System::default_output_device() {
                if let Ok(uid) = current.uid() {
                    if !uid.equal(&output_uid) {
                        break Err(anyhow!("default output device changed"));
                    }
                }
            }
        };

        // Teardown on this thread, in order: stop IO, then guards drop
        // (StartedDevice → proc id → aggregate device → tap).
        let _ = started.stop();
        result
    }
}
