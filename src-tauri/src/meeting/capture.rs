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
use std::{io::Write, path::Path};

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
    /// Total mono samples ever pushed (across session rebuilds). The tap
    /// watchdog checks it against wall time × declared rate: a stream can't
    /// legitimately deliver faster than real time, so sustained over-delivery
    /// proves the format is being misread (48 kHz/stereo taken for 16 kHz/mono
    /// turns speech into rumble that whisper hallucinates over).
    pub pushed_total: AtomicU64,
}

impl ChannelBuf {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            samples: Mutex::new(Vec::new()),
            sample_rate: AtomicU32::new(0),
            last_callback: AtomicU64::new(0),
            last_signal: AtomicU64::new(0),
            pushed_total: AtomicU64::new(0),
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
        self.pushed_total
            .fetch_add(mono.len() as u64, Ordering::Relaxed);
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

/// Number of mono frames represented by a Core Audio callback buffer. The
/// callback's byte count and live channel count are authoritative; deriving
/// this through an AVAudioPCMBuffer built from a cached startup format is what
/// previously stretched remote audio when VoiceProcessingIO changed formats.
fn callback_mono_frames(byte_size: u32, channels: usize) -> Option<usize> {
    if channels == 0 || byte_size as usize % std::mem::size_of::<f32>() != 0 {
        return None;
    }
    let samples = byte_size as usize / std::mem::size_of::<f32>();
    (samples % channels == 0).then_some(samples / channels)
}

fn callback_overdelivers(frames: usize, previous_sample_time: f64, sample_time: f64) -> bool {
    let clock_frames = sample_time - previous_sample_time;
    clock_frames > 0.0 && frames as f64 > clock_frames * 1.25 + 4.0
}

fn infer_callback_layout(
    total_samples: usize,
    previous_sample_time: f64,
    sample_time: f64,
) -> Option<(usize, usize)> {
    let clock_frames = sample_time - previous_sample_time;
    let frames = clock_frames.round() as usize;
    (clock_frames > 0.0
        && (clock_frames - frames as f64).abs() < 0.01
        && frames > 0
        && total_samples % frames == 0)
        .then_some((frames, total_samples / frames))
        .filter(|(_, channels)| (1..=32).contains(channels))
}

fn capture_log(path: Option<&Path>, message: &str) {
    let Some(path) = path else { return };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let now = chrono::Utc::now().to_rfc3339();
        let _ = writeln!(file, "{now} {message}");
    }
}

// ---------------------------------------------------------------------------
// Mic capture, on its own thread. Preferred path (macOS, `aec` on): Apple's
// VoiceProcessingIO AudioUnit — the OS subtracts everything the Mac is
// playing (i.e. the call's remote audio) from the mic signal, which kills
// speaker echo at the source for no-headphones calls. Falls back to a plain
// cpal stream when VPIO can't initialize (odd devices, denied component).
// ---------------------------------------------------------------------------

pub fn run_mic(buf: Arc<ChannelBuf>, stop: Arc<AtomicBool>, aec: bool) {
    while !stop.load(Ordering::Relaxed) {
        let result = if aec && cfg!(target_os = "macos") {
            #[cfg(target_os = "macos")]
            {
                match vp::vp_session(&buf, &stop) {
                    Err(e) if !vp::started(&e) => {
                        // VPIO never came up — this session runs on raw cpal;
                        // the next rebuild tries VPIO again.
                        eprintln!("[noted] mic AEC unavailable, using raw mic: {e}");
                        mic_session(&buf, &stop)
                    }
                    r => r,
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                mic_session(&buf, &stop)
            }
        } else {
            mic_session(&buf, &stop)
        };
        match result {
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
// Mic capture via VoiceProcessingIO (macOS): input-only AUVoiceIO with the
// default output device as the echo-cancellation reference.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod vp {
    use super::*;
    use cidre::{at::au, cat, core_audio as ca, os};

    /// Errors after "started:" mean VPIO ran and then broke (device change,
    /// stall) — rebuild VPIO. Anything else is an init failure — fall back.
    const STARTED: &str = "started:";

    pub fn started(e: &anyhow::Error) -> bool {
        e.to_string().starts_with(STARTED)
    }

    struct Ctx {
        buf: Arc<ChannelBuf>,
        /// The opaque AudioComponentInstance; stable across Output moves.
        /// Null until resources are allocated — callbacks can't fire before
        /// start(), but guard anyway.
        unit: *mut au::Unit,
        scratch: Vec<f32>,
        render_err: Arc<AtomicBool>,
        callbacks: u64,
    }

    extern "C-unwind" fn input_cb(
        ctx: *mut Ctx,
        _flags: &mut au::RenderActionFlags,
        ts: &cat::AudioTimeStamp,
        bus: u32,
        n_frames: u32,
        _io_data: *mut cat::AudioBufList<1>,
    ) -> os::Status {
        let Some(ctx) = (unsafe { ctx.as_mut() }) else {
            return os::Status::NO_ERR;
        };
        if ctx.unit.is_null() {
            return os::Status::NO_ERR;
        }
        ctx.callbacks += 1;
        let n = n_frames as usize;
        if ctx.scratch.len() < n {
            ctx.scratch.resize(n, 0.0);
        }
        let mut list = cat::AudioBufList::<1> {
            number_buffers: 1,
            buffers: [cat::audio::Buf {
                number_channels: 1,
                data_bytes_size: (n * std::mem::size_of::<f32>()) as u32,
                data: ctx.scratch.as_mut_ptr() as *mut u8,
            }],
        };
        let unit = unsafe { &mut *ctx.unit };
        match unit.render(ts, bus, n_frames, &mut list) {
            Ok(()) => {
                ctx.buf.push(&ctx.scratch[..n]);
                if ctx.callbacks == 1 {
                    eprintln!("[noted] mic vp: first callback, {n} samples");
                }
                os::Status::NO_ERR
            }
            Err(e) => {
                ctx.render_err.store(true, Ordering::Relaxed);
                e.status()
            }
        }
    }

    /// One VPIO lifetime: build → run until stop / stall / device change.
    /// Ok(()) = clean stop; Err("started:…") = rebuild; other Err = fall back.
    pub fn vp_session(buf: &Arc<ChannelBuf>, stop: &Arc<AtomicBool>) -> Result<()> {
        let e = |what: &'static str| move |err| anyhow!("vp {what}: {err:?}");

        // The callback reads Ctx behind a raw pointer, so it must be declared
        // before (= dropped after) the Output that drives the callbacks.
        let render_err = Arc::new(AtomicBool::new(false));
        let mut ctx = Box::new(Ctx {
            buf: buf.clone(),
            unit: std::ptr::null_mut(),
            scratch: vec![0.0; 8192],
            render_err: render_err.clone(),
            callbacks: 0,
        });

        let mut output = au::Output::new_apple_vp().map_err(e("open"))?;
        output
            .set_io_enabled(au::Scope::INPUT, 1, true)
            .map_err(e("enable input"))?;
        output
            .set_io_enabled(au::Scope::OUTPUT, 0, false)
            .map_err(e("disable output"))?;

        // VPIO's default behavior DUCKS all other audio while voice is
        // detected — that would turn the actual call down under the user.
        // Best-effort: the property is newer than the unit itself.
        let duck = au::VoiceIoOtherAudioDuckingCfg {
            enable_advanced_ducking: false,
            ducking_level: au::voice_io_other_audio_ducking_level::MIN,
        };
        if let Err(err) = output.vp_set_other_audio_ducking_cfg(&duck) {
            eprintln!("[noted] mic vp: ducking cfg not applied ({err:?})");
        }
        // Natural levels: the ASR energy gate (ChunkerCfg thresholds) was
        // tuned on a raw mic; AGC pumping would move the floor under it.
        if let Err(err) = output.vp_set_enable_agc(false) {
            eprintln!("[noted] mic vp: agc off not applied ({err:?})");
        }

        let input_device = ca::System::default_input_device().map_err(e("input device"))?;
        output
            .set_input_device(&input_device)
            .map_err(e("bind input"))?;
        let input_uid = input_device.uid().map_err(e("input uid"))?;
        // The echo reference is the default output device (what the call
        // plays through); if it changes (AirPods!) we rebuild to re-anchor.
        let output_uid = ca::System::default_output_device()
            .and_then(|d| d.uid())
            .map_err(e("output uid"))?;

        // Client format: mono f32 at the unit's own rate — the ASR worker
        // resamples, so no rate negotiation to get wrong.
        let fmt = output.input_stream_format(1).map_err(e("format"))?;
        let desired = cat::audio::StreamBasicDesc {
            sample_rate: fmt.sample_rate,
            format: cat::audio::Format::LINEAR_PCM,
            format_flags: cat::audio::FormatFlags::IS_FLOAT
                | cat::audio::FormatFlags::IS_PACKED
                | cat::audio::FormatFlags::IS_NON_INTERLEAVED,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels_per_frame: 1,
            bits_per_channel: 32,
            reserved: 0,
        };
        output
            .set_input_stream_format(&desired)
            .map_err(e("set format"))?;
        output
            .set_input_cb(input_cb, &*ctx as *const Ctx)
            .map_err(e("callback"))?;

        let mut output = output.allocate_resources().map_err(e("init"))?;
        ctx.unit = output.unit_mut() as *mut au::Unit;
        buf.sample_rate
            .store(fmt.sample_rate as u32, Ordering::Relaxed);
        output.start().map_err(e("start"))?;
        eprintln!(
            "[noted] mic: VoiceProcessingIO running ({} Hz, AEC on)",
            fmt.sample_rate as u32
        );

        buf.last_callback.store(epoch_ms(), Ordering::Relaxed);
        let result = loop {
            if stop.load(Ordering::Relaxed) {
                break Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));

            if render_err.load(Ordering::Relaxed) {
                break Err(anyhow!("{STARTED} render error"));
            }
            let stale_ms = epoch_ms().saturating_sub(buf.last_callback.load(Ordering::Relaxed));
            if stale_ms > 10_000 {
                break Err(anyhow!("{STARTED} no callbacks for {stale_ms}ms"));
            }
            if let Ok(uid) = ca::System::default_input_device().and_then(|d| d.uid()) {
                if !uid.equal(&input_uid) {
                    break Err(anyhow!("{STARTED} default input device changed"));
                }
            }
            if let Ok(uid) = ca::System::default_output_device().and_then(|d| d.uid()) {
                if !uid.equal(&output_uid) {
                    break Err(anyhow!("{STARTED} default output device changed"));
                }
            }
        };
        let _ = output.stop();
        eprintln!(
            "[noted] mic vp session ended: {} callbacks ({})",
            ctx.callbacks,
            if result.is_ok() {
                "clean stop"
            } else {
                "rebuilding"
            }
        );
        result
    }
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
pub fn run_system_tap(
    buf: Arc<ChannelBuf>,
    stop: Arc<AtomicBool>,
    log_path: Option<std::path::PathBuf>,
) {
    while !stop.load(Ordering::Relaxed) {
        match macos::tap_session(&buf, &stop, log_path.as_deref()) {
            Ok(()) => break, // clean stop
            Err(e) => {
                eprintln!("[noted] system tap error (rebuilding in 2s): {e}");
                capture_log(log_path.as_deref(), &format!("tap rebuild: {e}"));
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
pub fn run_system_tap(
    _buf: Arc<ChannelBuf>,
    _stop: Arc<AtomicBool>,
    _log_path: Option<std::path::PathBuf>,
) {
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use cidre::core_audio::aggregate_device_keys as agg_keys;
    use cidre::core_audio::sub_device_keys as sub_keys;
    use cidre::{cat, cf, core_audio as ca, ns, os};

    struct Ctx {
        buf: Arc<ChannelBuf>,
        channels: usize,
        last_sample_time: Option<f64>,
        last_rate_clock: Option<(f64, u64)>,
        host_clock_hz: f64,
        format_mismatches: Arc<AtomicU64>,
        callbacks: u64,
        pushed_frames: u64,
        first_bytes: u32,
        max_bytes: u32,
        invalid_callbacks: u64,
        channel_changes: u64,
        clock_layout_corrections: u64,
        rate_changes: u64,
    }

    extern "C" fn io_proc(
        _device: ca::Device,
        _now: &cat::AudioTimeStamp,
        input_data: &cat::AudioBufList<1>,
        input_time: &cat::AudioTimeStamp,
        _output_data: &mut cat::AudioBufList<1>,
        _output_time: &cat::AudioTimeStamp,
        ctx: Option<&mut Ctx>,
    ) -> os::Status {
        let Some(ctx) = ctx else {
            return Default::default();
        };
        ctx.callbacks += 1;
        // Read exactly mDataByteSize from the callback. Do not wrap this in an
        // AVAudioPCMBuffer using the tap's creation-time format: after VPIO
        // reconfigures the output device, that wrapper can report a stale frame
        // length and expose several times the valid samples.
        let b = &input_data.buffers[0];
        if ctx.callbacks == 1 {
            ctx.first_bytes = b.data_bytes_size;
        }
        ctx.max_bytes = ctx.max_bytes.max(b.data_bytes_size);
        let header_channels = b.number_channels as usize;
        let Some(mut frames) = callback_mono_frames(b.data_bytes_size, header_channels) else {
            ctx.invalid_callbacks += 1;
            return Default::default();
        };
        let total_samples = b.data_bytes_size as usize / std::mem::size_of::<f32>();
        let mut channels = header_channels;
        let previous_sample_time = ctx.last_sample_time;
        // Core Audio's sample clock gives an independent frame count. If the
        // buffer header lies about its live channel layout, infer the actual
        // interleave count and downmix correctly. This is the Discord failure:
        // the header stayed mono while four channels' bytes were delivered.
        if input_time.flags.0 & cat::AudioTimeStampFlags::SAMPLE_TIME_VALID.0 != 0 {
            if let Some(previous) = previous_sample_time {
                if let Some((clock_frames, inferred)) =
                    infer_callback_layout(total_samples, previous, input_time.sample_time)
                {
                    frames = clock_frames;
                    channels = inferred;
                    if inferred != header_channels {
                        ctx.clock_layout_corrections += 1;
                    }
                } else if callback_overdelivers(frames, previous, input_time.sample_time) {
                    ctx.format_mismatches.fetch_add(1, Ordering::Relaxed);
                    ctx.last_sample_time = Some(input_time.sample_time);
                    return Default::default();
                }
            }
        }

        // The same timestamps reveal a live sample-rate change. Keep the
        // resampler synchronized instead of trusting the creation-time ASBD.
        let time_flags = cat::AudioTimeStampFlags::SAMPLE_HOST_TIME_VALID.0;
        if input_time.flags.0 & time_flags == time_flags {
            if let Some((previous_sample, previous_host)) = ctx.last_rate_clock {
                let sample_delta = input_time.sample_time - previous_sample;
                let host_delta = input_time.host_time.saturating_sub(previous_host);
                if sample_delta > 0.0 && host_delta > 0 {
                    let measured = sample_delta * ctx.host_clock_hz / host_delta as f64;
                    if measured.is_finite() && (8_000.0..=384_000.0).contains(&measured) {
                        let measured = measured.round() as u32;
                        let current = ctx.buf.sample_rate.load(Ordering::Relaxed);
                        if current.abs_diff(measured) > current.max(1) / 100 {
                            ctx.buf.sample_rate.store(measured, Ordering::Relaxed);
                            ctx.rate_changes += 1;
                        }
                    }
                }
            }
            ctx.last_rate_clock = Some((input_time.sample_time, input_time.host_time));
        }
        if input_time.flags.0 & cat::AudioTimeStampFlags::SAMPLE_TIME_VALID.0 != 0 {
            ctx.last_sample_time = Some(input_time.sample_time);
        }

        if channels != ctx.channels {
            eprintln!(
                "[noted] tap: effective channel count changed {} -> {channels} (header={header_channels})",
                ctx.channels
            );
            ctx.channels = channels;
            ctx.channel_changes += 1;
        }
        if frames > 0 && !b.data.is_null() {
            let data = unsafe { std::slice::from_raw_parts(b.data as *const f32, total_samples) };
            ctx.pushed_frames += frames as u64;
            if channels == 1 {
                ctx.buf.push(data);
            } else {
                ctx.buf.push(&downmix_mono(data, channels));
            }
        }
        if ctx.callbacks == 1 {
            eprintln!(
                "[noted] tap: first callback, {} bytes / {frames} mono frames / {} ch",
                b.data_bytes_size, channels
            );
        }
        Default::default()
    }

    /// One tap lifetime: build → run until stop / stall / device change.
    /// Ok(()) = clean stop; Err = rebuild wanted.
    pub fn tap_session(
        buf: &Arc<ChannelBuf>,
        stop: &Arc<AtomicBool>,
        log_path: Option<&Path>,
    ) -> Result<()> {
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
        if asbd.bits_per_channel != 32
            || !asbd.format_flags.contains(cat::AudioFormatFlags::IS_FLOAT)
            || asbd.channels_per_frame == 0
            || !asbd.sample_rate.is_finite()
            || asbd.sample_rate <= 0.0
        {
            return Err(anyhow!("unsupported tap PCM format: {asbd:?}"));
        }
        buf.sample_rate
            .store(asbd.sample_rate as u32, Ordering::Relaxed);
        let channels = asbd.channels_per_frame as usize;
        eprintln!(
            "[noted] tap created: {} Hz, {} ch, {} bits",
            asbd.sample_rate, channels, asbd.bits_per_channel
        );
        capture_log(
            log_path,
            &format!(
                "tap created: {} Hz, {} ch, {} bits, flags={:?}",
                asbd.sample_rate, channels, asbd.bits_per_channel, asbd.format_flags
            ),
        );

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

        let format_mismatches = Arc::new(AtomicU64::new(0));
        let mut ctx = Ctx {
            buf: buf.clone(),
            channels,
            last_sample_time: None,
            last_rate_clock: None,
            host_clock_hz: cidre::cv::host_clock_frequency(),
            format_mismatches: format_mismatches.clone(),
            callbacks: 0,
            pushed_frames: 0,
            first_bytes: 0,
            max_bytes: 0,
            invalid_callbacks: 0,
            channel_changes: 0,
            clock_layout_corrections: 0,
            rate_changes: 0,
        };
        let proc_id = agg_device
            .create_io_proc_id(io_proc, Some(&mut ctx))
            .map_err(|e| anyhow!("io proc: {e:?}"))?;
        let started = ca::device_start(&*agg_device, Some(proc_id))
            .map_err(|e| anyhow!("device start: {e:?}"))?;

        // Poll loop: clean stop, watchdog stall, format drift, or output-device
        // switch.
        buf.last_callback.store(epoch_ms(), Ordering::Relaxed);
        let session_t0 = epoch_ms();
        let pushed_t0 = buf.pushed_total.load(Ordering::Relaxed);
        let result = loop {
            if stop.load(Ordering::Relaxed) {
                break Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));

            let mismatches = format_mismatches.load(Ordering::Relaxed);
            if mismatches > 0 {
                break Err(anyhow!(
                    "tap callback frame count disagreed with the Core Audio clock ({mismatches} callbacks rejected)"
                ));
            }

            let stale_ms = epoch_ms().saturating_sub(buf.last_callback.load(Ordering::Relaxed));
            if stale_ms > 10_000 {
                break Err(anyhow!("tap delivered no callbacks for {stale_ms}ms"));
            }
            // Format-drift watchdog: more samples than wall time × declared
            // rate is physically impossible for a live stream, so the declared
            // rate is stale (device reconfigured under us) — rebuild to pick
            // up the real format. Under-delivery is NOT checked: the tap
            // legitimately goes quiet whenever no app plays audio.
            let elapsed_ms = epoch_ms().saturating_sub(session_t0);
            if elapsed_ms > 2_000 {
                let pushed = buf.pushed_total.load(Ordering::Relaxed) - pushed_t0;
                let live_rate = buf.sample_rate.load(Ordering::Relaxed).max(1) as u64;
                let expected = live_rate * elapsed_ms / 1000;
                if pushed > expected + expected / 4 {
                    break Err(anyhow!(
                        "tap over-delivering ({pushed} samples in {elapsed_ms}ms at {live_rate} Hz) — stream format changed"
                    ));
                }
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
        eprintln!(
            "[noted] tap session ended: {} callbacks, {} mono frames pushed ({})",
            ctx.callbacks,
            ctx.pushed_frames,
            if result.is_ok() {
                "clean stop"
            } else {
                "rebuilding"
            }
        );
        capture_log(
            log_path,
            &format!(
                "tap ended: callbacks={}, mono_frames={}, first_bytes={}, max_bytes={}, channels={}, channel_changes={}, clock_layout_corrections={}, rate_changes={}, invalid_callbacks={}, format_mismatches={}, result={}",
                ctx.callbacks,
                ctx.pushed_frames,
                ctx.first_bytes,
                ctx.max_bytes,
                ctx.channels,
                ctx.channel_changes,
                ctx.clock_layout_corrections,
                ctx.rate_changes,
                ctx.invalid_callbacks,
                ctx.format_mismatches.load(Ordering::Relaxed),
                if result.is_ok() { "clean" } else { "rebuild" },
            ),
        );
        result
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn callback_frames_use_live_bytes_and_channels() {
        // 512 stereo f32 frames = 4096 bytes. A stale mono format would call
        // this 1024 frames and reproduce the double-speed/over-delivery bug.
        assert_eq!(callback_mono_frames(4096, 2), Some(512));
        assert_eq!(callback_mono_frames(2048, 1), Some(512));
        assert_eq!(callback_mono_frames(2047, 1), None);
        assert_eq!(callback_mono_frames(2048, 0), None);
        assert!(!callback_overdelivers(512, 1_000.0, 1_512.0));
        assert!(callback_overdelivers(2_048, 1_000.0, 1_512.0));
        assert_eq!(infer_callback_layout(512, 1_000.0, 1_512.0), Some((512, 1)));
        assert_eq!(
            infer_callback_layout(2_048, 1_000.0, 1_512.0),
            Some((512, 4))
        );
    }

    /// Live smoke test: builds a real VoiceProcessingIO session, records 3s,
    /// and asserts callbacks arrived. Grabs the actual microphone (and needs
    /// the host terminal's mic permission), so it stays ignored:
    ///   cargo test --lib vp_smoke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn vp_smoke() {
        let buf = ChannelBuf::new();
        let stop = Arc::new(AtomicBool::new(false));
        let (b, s) = (buf.clone(), stop.clone());
        let t = std::thread::spawn(move || vp::vp_session(&b, &s));
        std::thread::sleep(Duration::from_secs(3));
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap().expect("vp session failed");
        let (samples, rate) = buf.drain();
        eprintln!("vp_smoke: {} samples @ {} Hz", samples.len(), rate);
        assert!(rate > 0, "no sample rate reported");
        assert!(
            samples.len() as u32 > rate, // > 1s of audio over a 3s run
            "too few samples: {}",
            samples.len()
        );
    }
}
