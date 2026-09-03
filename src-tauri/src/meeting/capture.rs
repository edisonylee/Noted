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
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use std::{io::Write, path::Path};

use anyhow::{anyhow, Result};

// VoiceProcessingIO and the system-audio aggregate both reconfigure CoreAudio.
// Starting them concurrently is usually tolerated, but route changes (Zoom +
// Bluetooth is the common case) can make both rebuild at once and leave the new
// aggregate half-initialized. Serialize only graph setup; capture still runs on
// its independent real-time threads after startup.
#[cfg(target_os = "macos")]
static AUDIO_GRAPH_SETUP: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(target_os = "macos")]
fn audio_graph_setup_lock() -> MutexGuard<'static, ()> {
    AUDIO_GRAPH_SETUP
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
        let mut buf = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if buf.len().saturating_add(mono.len()) > cap {
            buf.clear();
        }
        buf.extend_from_slice(mono);
    }

    /// Take everything accumulated since the last drain.
    pub fn drain(&self) -> (Vec<f32>, u32) {
        let rate = self.sample_rate.load(Ordering::Relaxed);
        let mut buf = self
            .samples
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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

fn vpio_needs_raw_fallback(elapsed_ms: u64, nonzero_callbacks: u64) -> bool {
    elapsed_ms > 2_000 && nonzero_callbacks == 0
}

// ---------------------------------------------------------------------------
// Mic capture, on its own thread. Preferred path (macOS, `aec` on): Apple's
// VoiceProcessingIO AudioUnit — the OS subtracts everything the Mac is
// playing (i.e. the call's remote audio) from the mic signal, which kills
// speaker echo at the source for no-headphones calls. Falls back to a plain
// cpal stream when VPIO can't initialize (odd devices, denied component).
// ---------------------------------------------------------------------------

/// How the microphone ended up being captured, so the caller can tell the user.
///
/// macOS voice processing (VoiceProcessingIO) gives us hardware echo
/// cancellation, but it seizes the input device: while it runs, any *other* app
/// on that mic records silence. Muting the user in their own call is a far worse
/// failure than a little speaker bleed on our transcript, so a live call always
/// wins the device and this reports which way it went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MicAec {
    /// Voice processing is on: echo cancelled, nothing else wanted the mic.
    Active,
    /// The user turned echo cancellation off in Settings.
    OffByChoice,
    /// Another app holds the mic, so we yielded the device and captured raw.
    YieldedTo { bundle: String },
    /// Voice processing was wanted but could not run — an odd input device, a
    /// denied audio component, or a session that produced only zeros. Captured
    /// raw, so the same speaker-bleed caveat applies.
    Unavailable,
}

impl MicAec {
    /// Whether this decision runs the VoiceProcessingIO path.
    pub fn uses_voice_processing(&self) -> bool {
        matches!(self, MicAec::Active)
    }
}

/// Decide how to capture the mic. Pure so the policy is testable without
/// CoreAudio: `call_apps` comes from `detect::call_apps_on_mic`.
pub fn decide_mic_aec(aec_requested: bool, call_apps: &[String]) -> MicAec {
    if !aec_requested {
        return MicAec::OffByChoice;
    }
    match call_apps.first() {
        Some(bundle) => MicAec::YieldedTo {
            bundle: bundle.clone(),
        },
        None => MicAec::Active,
    }
}

/// Everything `run_mic` needs to pick and re-evaluate a capture strategy.
/// Plain data: no config or Tauri types reach the audio threads.
#[derive(Debug, Clone, Default)]
pub struct MicPlan {
    /// The user's echo-cancellation preference (Settings -> Meetings).
    pub aec_requested: bool,
    /// Bundles that are never call apps (`MeetingsCfg::ignore_bundles`).
    pub ignore_bundles: Vec<String>,
}

impl MicPlan {
    pub fn new(aec_requested: bool, ignore_bundles: Vec<String>) -> Self {
        Self {
            aec_requested,
            ignore_bundles,
        }
    }

    /// The decision as of right now.
    fn decide(&self) -> MicAec {
        decide_mic_aec(
            self.aec_requested,
            &super::detect::call_apps_on_mic(&self.ignore_bundles),
        )
    }
}

/// Notified whenever the capture strategy is settled or changes mid-recording,
/// so the caller can surface it. Kept as a callback so this module stays free of
/// Tauri and of any opinion about how the user is told.
pub type AecNotify = Arc<dyn Fn(MicAec) + Send + Sync>;

pub fn run_mic(
    buf: Arc<ChannelBuf>,
    stop: Arc<AtomicBool>,
    plan: MicPlan,
    log_path: Option<std::path::PathBuf>,
    notify: Option<AecNotify>,
) {
    // Report the opening decision once, then again only when it changes, so the
    // UI is not re-notified on every internal session rebuild.
    let mut announced: Option<MicAec> = None;
    let mut announce = |decision: &MicAec| {
        if announced.as_ref() == Some(decision) {
            return;
        }
        announced = Some(decision.clone());
        capture_log(log_path.as_deref(), &format!("mic capture: {decision:?}"));
        if let Some(notify) = notify.as_ref() {
            notify(decision.clone());
        }
    };

    while !stop.load(Ordering::Relaxed) {
        let decision = plan.decide();
        announce(&decision);
        let result = if decision.uses_voice_processing() && cfg!(target_os = "macos") {
            #[cfg(target_os = "macos")]
            {
                match vp::vp_session(&buf, &stop, &plan.ignore_bundles) {
                    Err(e) if !vp::started(&e) => {
                        // VPIO either failed to start, produced an all-zero
                        // stream, or yielded the device to a call app that
                        // joined mid-recording. Never let a healthy-looking
                        // callback loop silently erase the user's mic channel,
                        // and never hold the device a live call needs.
                        eprintln!("[noted] mic AEC unavailable, using raw mic: {e}");
                        capture_log(
                            log_path.as_deref(),
                            &format!("mic AEC unavailable; raw fallback: {e}"),
                        );
                        // Report what is actually running now. A call app
                        // taking the mic is a yield; anything else means voice
                        // processing simply could not run — never re-announce
                        // it as Active while the raw mic is what is recording.
                        announce(&match plan.decide() {
                            yielded @ MicAec::YieldedTo { .. } => yielded,
                            _ => MicAec::Unavailable,
                        });
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
                capture_log(log_path.as_deref(), &format!("mic capture retry: {e}"));
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

    /// How often to re-check whether a call app has taken the mic. Enumerating
    /// CoreAudio processes is much heavier than the rest of the watchdog tick.
    const CALL_SCAN_MS: u64 = 1_000;

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
        nonzero_callbacks: Arc<AtomicU64>,
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
                if ctx.scratch[..n].iter().any(|sample| *sample != 0.0) {
                    ctx.nonzero_callbacks.fetch_add(1, Ordering::Relaxed);
                }
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
    pub fn vp_session(
        buf: &Arc<ChannelBuf>,
        stop: &Arc<AtomicBool>,
        ignore_bundles: &[String],
    ) -> Result<()> {
        let e = |what: &'static str| move |err| anyhow!("vp {what}: {err:?}");

        // A call that starts after we do would otherwise capture silence: VPIO
        // already holds the input device. Bail before touching the device at
        // all — the error is deliberately not {STARTED}-class so run_mic falls
        // straight through to the raw mic instead of rebuilding VPIO.
        if let Some(bundle) = super::super::detect::call_apps_on_mic(ignore_bundles).first() {
            return Err(anyhow!("yielding mic to {bundle}"));
        }

        // Do not race the system-tap aggregate while Zoom/CoreAudio is moving
        // between speaker, headset, and Bluetooth call profiles.
        let setup_guard = audio_graph_setup_lock();

        // The callback reads Ctx behind a raw pointer, so it must be declared
        // before (= dropped after) the Output that drives the callbacks.
        let render_err = Arc::new(AtomicBool::new(false));
        let nonzero_callbacks = Arc::new(AtomicU64::new(0));
        let mut ctx = Box::new(Ctx {
            buf: buf.clone(),
            unit: std::ptr::null_mut(),
            scratch: vec![0.0; 8192],
            render_err: render_err.clone(),
            nonzero_callbacks: nonzero_callbacks.clone(),
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
        drop(setup_guard);
        eprintln!(
            "[noted] mic: VoiceProcessingIO running ({} Hz, AEC on)",
            fmt.sample_rate as u32
        );

        buf.last_callback.store(epoch_ms(), Ordering::Relaxed);
        let session_started = epoch_ms();
        let mut last_call_scan = epoch_ms();
        let result = loop {
            if stop.load(Ordering::Relaxed) {
                break Ok(());
            }
            std::thread::sleep(Duration::from_millis(250));

            if render_err.load(Ordering::Relaxed) {
                break Err(anyhow!("{STARTED} render error"));
            }
            // A real microphone has a non-zero noise floor even when nobody
            // is speaking. Two seconds of callbacks containing exact zeros is
            // a broken VPIO route, not silence; return an unstarted-class error
            // so run_mic immediately falls back to the raw input stream.
            if vpio_needs_raw_fallback(
                epoch_ms().saturating_sub(session_started),
                nonzero_callbacks.load(Ordering::Relaxed),
            ) {
                break Err(anyhow!("vp produced only zero-valued mic samples"));
            }
            let stale_ms = epoch_ms().saturating_sub(buf.last_callback.load(Ordering::Relaxed));
            if stale_ms > 10_000 {
                break Err(anyhow!("{STARTED} no callbacks for {stale_ms}ms"));
            }
            // A call app joined the mic after we started. We are holding the
            // input device it needs, so it is recording silence right now —
            // release it. Not {STARTED}-class: run_mic must fall back to the
            // raw mic, not rebuild VPIO and seize the device again.
            //
            // Enumerating CoreAudio processes is far heavier than the atomics
            // around it, so poll it about once a second rather than on every
            // 250ms tick; the pre-flight check already covers the common case
            // of the call being up before we start.
            if epoch_ms().saturating_sub(last_call_scan) >= CALL_SCAN_MS {
                last_call_scan = epoch_ms();
                if let Some(bundle) = super::super::detect::call_apps_on_mic(ignore_bundles).first()
                {
                    break Err(anyhow!("yielding mic to {bundle}"));
                }
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
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                eprintln!("[noted] system tap error (waiting for audio route): {e}");
                capture_log(log_path.as_deref(), &format!("tap rebuild: {e}"));
                // VPIO retries after 2s. Give it a short head start so the mic
                // graph establishes the final Zoom route before the tap binds.
                for _ in 0..10 {
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
        callback_panicked: Arc<AtomicBool>,
        callbacks: u64,
        pushed_frames: u64,
        first_bytes: u32,
        max_bytes: u32,
        invalid_callbacks: u64,
        channel_changes: u64,
        clock_layout_corrections: u64,
        rate_changes: u64,
    }

    fn io_proc_inner(
        input_data: &cat::AudioBufList<1>,
        input_time: &cat::AudioTimeStamp,
        ctx: &mut Ctx,
    ) -> os::Status {
        ctx.callbacks = ctx.callbacks.saturating_add(1);
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
            ctx.invalid_callbacks = ctx.invalid_callbacks.saturating_add(1);
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
                        ctx.clock_layout_corrections =
                            ctx.clock_layout_corrections.saturating_add(1);
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
                            ctx.rate_changes = ctx.rate_changes.saturating_add(1);
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
            ctx.channel_changes = ctx.channel_changes.saturating_add(1);
        }
        if frames > 0 {
            // CoreAudio normally provides aligned Float32 data, but a device
            // being rebuilt can briefly violate that contract. Creating a Rust
            // slice from a null or misaligned foreign pointer aborts instead of
            // unwinding, so reject the callback before touching the pointer.
            if !audio_data_is_aligned(b.data) {
                ctx.invalid_callbacks = ctx.invalid_callbacks.saturating_add(1);
                return Default::default();
            }
            let data = unsafe { std::slice::from_raw_parts(b.data as *const f32, total_samples) };
            ctx.pushed_frames = ctx.pushed_frames.saturating_add(frames as u64);
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

    fn audio_data_is_aligned(data: *mut u8) -> bool {
        !data.is_null() && (data as usize) % std::mem::align_of::<f32>() == 0
    }

    fn contain_callback_panic(
        callback_panicked: &Arc<AtomicBool>,
        callback: impl FnOnce() -> os::Status,
    ) -> os::Status {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback)) {
            Ok(status) => status,
            Err(_) => {
                callback_panicked.store(true, Ordering::Relaxed);
                Default::default()
            }
        }
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
        if ctx.callback_panicked.load(Ordering::Relaxed) {
            return Default::default();
        }
        let callback_panicked = ctx.callback_panicked.clone();
        contain_callback_panic(&callback_panicked, || {
            io_proc_inner(input_data, input_time, ctx)
        })
    }

    fn wait_for_stable_output_device(stop: &AtomicBool) -> Result<ca::Device> {
        let mut candidate = ca::System::default_output_device()
            .map_err(|e| anyhow!("no default output device: {e:?}"))?;
        let mut candidate_uid = candidate.uid().map_err(|e| anyhow!("output uid: {e:?}"))?;
        let mut stable_since = std::time::Instant::now();
        let deadline = stable_since + Duration::from_secs(8);

        while std::time::Instant::now() < deadline {
            if stop.load(Ordering::Relaxed) {
                return Err(anyhow!("capture stopped while audio route was settling"));
            }
            if let Ok(current) = ca::System::default_output_device() {
                if let Ok(uid) = current.uid() {
                    if !uid.equal(&candidate_uid) {
                        candidate = current;
                        candidate_uid = uid;
                        stable_since = std::time::Instant::now();
                    } else {
                        candidate = current;
                    }

                    if stable_since.elapsed() >= Duration::from_millis(750) {
                        let alive = candidate.is_alive().unwrap_or(false);
                        let rate = candidate.nominal_sample_rate().unwrap_or(0.0);
                        let has_output = candidate
                            .output_stream_cfg()
                            .map(|cfg| cfg.number_buffers() > 0)
                            .unwrap_or(false);
                        if alive && rate.is_finite() && rate > 0.0 && has_output {
                            return Ok(candidate);
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        Err(anyhow!("default output device did not become ready"))
    }

    #[cfg(test)]
    mod callback_tests {
        use super::*;

        #[test]
        fn callback_panics_are_quarantined_before_the_c_boundary() {
            let panicked = Arc::new(AtomicBool::new(false));
            let _ = contain_callback_panic(&panicked, || panic!("simulated callback failure"));
            assert!(panicked.load(Ordering::Relaxed));
        }

        #[test]
        fn callback_rejects_null_and_misaligned_float_buffers() {
            let mut samples = [0.0_f32; 2];
            let aligned = samples.as_mut_ptr() as *mut u8;
            assert!(audio_data_is_aligned(aligned));
            assert!(!audio_data_is_aligned(std::ptr::null_mut()));
            assert!(!audio_data_is_aligned(unsafe { aligned.add(1) }));
        }
    }

    /// One tap lifetime: build → run until stop / stall / device change.
    /// Ok(()) = clean stop; Err = rebuild wanted.
    pub fn tap_session(
        buf: &Arc<ChannelBuf>,
        stop: &Arc<AtomicBool>,
        log_path: Option<&Path>,
    ) -> Result<()> {
        let output_device = wait_for_stable_output_device(stop)?;
        let output_uid = output_device
            .uid()
            .map_err(|e| anyhow!("output uid: {e:?}"))?;

        // The route is stable. Hold the setup gate while creating and starting
        // the aggregate so VPIO cannot reconfigure the same device underneath.
        let setup_guard = audio_graph_setup_lock();
        let current_uid = ca::System::default_output_device()
            .and_then(|device| device.uid())
            .map_err(|e| anyhow!("output route changed before tap setup: {e:?}"))?;
        if !current_uid.equal(&output_uid) {
            return Err(anyhow!("default output device changed before tap setup"));
        }

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
        let callback_panicked = Arc::new(AtomicBool::new(false));
        let mut ctx = Ctx {
            buf: buf.clone(),
            channels,
            last_sample_time: None,
            last_rate_clock: None,
            host_clock_hz: cidre::cv::host_clock_frequency(),
            format_mismatches: format_mismatches.clone(),
            callback_panicked: callback_panicked.clone(),
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
        drop(setup_guard);

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

            if callback_panicked.load(Ordering::Relaxed) {
                break Err(anyhow!("tap callback panicked; rebuilding safely"));
            }

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
                "tap ended: callbacks={}, mono_frames={}, first_bytes={}, max_bytes={}, channels={}, channel_changes={}, clock_layout_corrections={}, rate_changes={}, invalid_callbacks={}, format_mismatches={}, callback_panicked={}, result={}",
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
                ctx.callback_panicked.load(Ordering::Relaxed),
                if result.is_ok() { "clean" } else { "rebuild" },
            ),
        );
        result
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn bundles(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn voice_processing_runs_only_when_nothing_else_holds_the_mic() {
        assert_eq!(decide_mic_aec(true, &[]), MicAec::Active);
        assert!(decide_mic_aec(true, &[]).uses_voice_processing());
    }

    #[test]
    fn a_live_call_always_wins_the_input_device() {
        // The regression this exists for: VoiceProcessingIO seizes the mic, so
        // enabling echo cancellation used to mute the user in their own call.
        let decision = decide_mic_aec(true, &bundles(&["us.zoom.xos"]));
        assert_eq!(
            decision,
            MicAec::YieldedTo {
                bundle: "us.zoom.xos".to_string()
            }
        );
        assert!(
            !decision.uses_voice_processing(),
            "must not hold the device a call needs"
        );
    }

    #[test]
    fn only_a_live_voice_processing_session_counts_as_echo_cancelled() {
        // Everything except Active runs the raw mic, so all of them carry the
        // speaker-bleed caveat the UI warns about.
        for decision in [
            MicAec::OffByChoice,
            MicAec::Unavailable,
            MicAec::YieldedTo {
                bundle: "us.zoom.xos".to_string(),
            },
        ] {
            assert!(!decision.uses_voice_processing(), "{decision:?}");
        }
        assert!(MicAec::Active.uses_voice_processing());
    }

    #[test]
    fn the_user_switch_wins_over_detection() {
        // Off means off: never quietly re-enable voice processing just because
        // no call happens to be running.
        assert_eq!(decide_mic_aec(false, &[]), MicAec::OffByChoice);
        assert_eq!(
            decide_mic_aec(false, &bundles(&["us.zoom.xos"])),
            MicAec::OffByChoice
        );
    }

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

    #[test]
    fn exact_zero_vpio_stream_falls_back_after_two_seconds() {
        assert!(!vpio_needs_raw_fallback(2_000, 0));
        assert!(vpio_needs_raw_fallback(2_001, 0));
        assert!(!vpio_needs_raw_fallback(10_000, 1));
    }

    #[test]
    fn channel_buffer_recovers_from_a_poisoned_audio_lock() {
        let buf = ChannelBuf::new();
        let poison_target = buf.clone();
        let _ = std::panic::catch_unwind(move || {
            let _guard = poison_target.samples.lock().unwrap();
            panic!("simulated worker failure while holding audio samples");
        });

        buf.sample_rate.store(48_000, Ordering::Relaxed);
        buf.push(&[0.1, -0.1]);
        let (samples, rate) = buf.drain();
        assert_eq!(rate, 48_000);
        assert_eq!(samples, vec![0.1, -0.1]);
    }

    /// Live smoke test: builds a real VoiceProcessingIO session, records 3s,
    /// and asserts callbacks arrived. Grabs the actual microphone (and needs
    /// the host terminal's mic permission), so it stays ignored:
    ///   cargo test --lib vp_smoke -- --ignored --nocapture
    ///
    /// Quit any call app first: the session now yields the input device to one
    /// rather than muting it, so a running Zoom makes this fail by design.
    #[test]
    #[ignore]
    fn vp_smoke() {
        let buf = ChannelBuf::new();
        let stop = Arc::new(AtomicBool::new(false));
        let (b, s) = (buf.clone(), stop.clone());
        let t = std::thread::spawn(move || vp::vp_session(&b, &s, &[]));
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
