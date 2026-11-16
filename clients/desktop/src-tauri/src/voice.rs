//! Dictation belongs to the client: microphone audio and recognition stay here.

pub mod models;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use models::Recognizer;
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicBool, AtomicI64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use tauri::{Emitter, Listener};

const MIC_ERROR: &str = "Zwiim can't use the microphone. Allow it in System Settings › Privacy & Security › Microphone.";

// Keep the core event plugin closed to preview frames. Like the LAN channels,
// voice subscriptions use commands guarded by lib.rs's app-origin check.
#[tauri::command]
pub fn voice_listen(
    app: tauri::AppHandle,
    event: String,
    on_event: tauri::ipc::Channel<serde_json::Value>,
) -> Result<u32, String> {
    if !["download", "level", "partial", "stopped"].contains(&event.as_str()) {
        return Err("Not a voice event.".into());
    }
    let target = app.clone();
    Ok(app.listen(format!("voice://{event}"), move |ev| {
        if let Ok(payload) = serde_json::from_str::<serde_json::Value>(ev.payload()) {
            if on_event.send(payload).is_err() {
                target.unlisten(ev.id());
            }
        }
    }))
}

#[tauri::command]
pub fn voice_unlisten(app: tauri::AppHandle, id: u32) {
    app.unlisten(id);
}

#[derive(Clone, Deserialize)]
pub struct VoiceOptions {
    model: String,
    language: String,
    input: String,
    gpu: bool,
    silence_secs: u32,
}

struct Loaded {
    model: String,
    language: String,
    gpu: bool,
    recognizer: Recognizer,
    used: Instant,
}

struct Core {
    loaded: Mutex<Option<Loaded>>,
    recording: Mutex<Option<Recording>>,
    downloads: Mutex<HashSet<String>>,
    keep_loaded: AtomicI64,
}

pub struct Voice(Arc<Core>);

impl Default for Voice {
    fn default() -> Self {
        let core = Arc::new(Core {
            loaded: Mutex::new(None),
            recording: Mutex::new(None),
            downloads: Mutex::new(HashSet::new()),
            keep_loaded: AtomicI64::new(900),
        });
        let weak = Arc::downgrade(&core);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            let Some(core) = weak.upgrade() else {
                break;
            };
            let Ok(recording) = core.recording.try_lock() else {
                continue;
            };
            if recording.is_some() {
                continue;
            }
            let Ok(mut loaded) = core.loaded.try_lock() else {
                continue;
            };
            let secs = core.keep_loaded.load(Ordering::Relaxed);
            if secs >= 0
                && loaded
                    .as_ref()
                    .is_some_and(|l| l.used.elapsed() >= Duration::from_secs(secs as u64))
            {
                *loaded = None;
            }
        });
        Self(core)
    }
}

#[derive(Default)]
struct Audio {
    samples: Vec<f32>,
    squared: f64,
    count: usize,
}

struct Recording {
    model: String,
    audio: Arc<Mutex<Audio>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Recording {
    fn close(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Drop for Recording {
    fn drop(&mut self) {
        self.close();
    }
}

/// Keeps the fractional position across microphone callbacks, including a sample at the boundary.
struct Resampler {
    pending: Vec<f32>,
    position: f64,
    step: f64,
}
impl Resampler {
    fn new(rate: u32) -> Self {
        Self {
            pending: Vec::new(),
            position: 0.0,
            step: rate as f64 / 16000.0,
        }
    }
    fn append(&mut self, mono: impl Iterator<Item = f32>, out: &mut Audio) {
        self.pending.extend(mono);
        while self.position.floor() as usize + 1 < self.pending.len() {
            let at = self.position.floor() as usize;
            let fraction = (self.position - at as f64) as f32;
            let sample = self.pending[at] * (1.0 - fraction) + self.pending[at + 1] * fraction;
            out.samples.push(sample);
            out.squared += (sample as f64).powi(2);
            out.count += 1;
            self.position += self.step;
        }
        let consumed = (self.position.floor() as usize).min(self.pending.len().saturating_sub(1));
        self.pending.drain(..consumed);
        self.position -= consumed as f64;
    }
}

#[derive(Default)]
struct Silence {
    samples: usize,
}
impl Silence {
    fn update(&mut self, rms: f32, samples: usize, secs: u32) -> bool {
        if rms < 0.008 {
            self.samples += samples;
        } else {
            self.samples = 0;
        }
        secs > 0 && self.samples >= secs as usize * 16000
    }
}

#[tauri::command]
pub fn voice_inputs() -> Result<Vec<String>, String> {
    cpal::default_host()
        .input_devices()
        .map(|devices| devices.map(|d| d.to_string()).collect())
        .map_err(|_| MIC_ERROR.into())
}

fn input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    audio: Arc<Mutex<Audio>>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = config.channels as usize;
    let mut resampler = Resampler::new(config.sample_rate);
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let mut audio = audio.lock().unwrap();
                let mono = data.chunks_exact(channels).map(|frame| {
                    frame
                        .iter()
                        .map(|s| <f32 as cpal::FromSample<T>>::from_sample_(*s))
                        .sum::<f32>()
                        / channels as f32
                });
                resampler.append(mono, &mut audio);
            },
            move |_| {
                failed.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(|_| MIC_ERROR.into())
}

fn open_mic(
    opts: &VoiceOptions,
    audio: Arc<Mutex<Audio>>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let device = if opts.input.is_empty() {
        host.default_input_device()
    } else {
        host.input_devices()
            .map_err(|_| MIC_ERROR)?
            .find(|d| d.to_string() == opts.input)
    }
    .ok_or("Zwiim can't find that microphone. Choose an input in Settings › Voice.")?;
    let supported = device.default_input_config().map_err(|_| MIC_ERROR)?;
    let config = supported.config();
    let stream = match supported.sample_format() {
        cpal::SampleFormat::I8 => input_stream::<i8>(&device, config, audio, failed),
        cpal::SampleFormat::I16 => input_stream::<i16>(&device, config, audio, failed),
        cpal::SampleFormat::I32 => input_stream::<i32>(&device, config, audio, failed),
        cpal::SampleFormat::I64 => input_stream::<i64>(&device, config, audio, failed),
        cpal::SampleFormat::U8 => input_stream::<u8>(&device, config, audio, failed),
        cpal::SampleFormat::U16 => input_stream::<u16>(&device, config, audio, failed),
        cpal::SampleFormat::U32 => input_stream::<u32>(&device, config, audio, failed),
        cpal::SampleFormat::U64 => input_stream::<u64>(&device, config, audio, failed),
        cpal::SampleFormat::F32 => input_stream::<f32>(&device, config, audio, failed),
        cpal::SampleFormat::F64 => input_stream::<f64>(&device, config, audio, failed),
        _ => Err("Zwiim can't record this microphone's audio format. Choose another input in Settings › Voice.".into()),
    }?;
    stream.play().map_err(|_| MIC_ERROR)?;
    Ok(stream)
}

fn capture(
    core: Arc<Core>,
    app: tauri::AppHandle,
    opts: VoiceOptions,
) -> Result<Recording, String> {
    let audio = Arc::new(Mutex::new(Audio::default()));
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let (samples, stopped) = (audio.clone(), stop.clone());
    let model = opts.model.clone();
    // cpal's stream stays on the thread that opened it; it is not Send on every platform.
    let thread = std::thread::spawn(move || {
        let failed = Arc::new(AtomicBool::new(false));
        let stream = match open_mic(&opts, samples.clone(), failed.clone()) {
            Ok(stream) => {
                let _ = ready_tx.send(Ok(()));
                stream
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        let mut silence = Silence::default();
        let mut last_decode = Instant::now();
        let mut decoding: Option<std::thread::JoinHandle<()>> = None;
        while !stopped.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(50));
            if stopped.load(Ordering::Acquire) {
                break;
            }
            if failed.load(Ordering::Acquire) {
                stopped.store(true, Ordering::Release);
                drop(stream);
                let _ = app.emit(
                    "voice://stopped",
                    serde_json::json!({"reason": "error", "error": MIC_ERROR}),
                );
                if let Some(worker) = decoding {
                    let _ = worker.join();
                }
                return;
            }
            let (rms, count) = {
                let mut audio = samples.lock().unwrap();
                let count = audio.count;
                let rms = if count == 0 {
                    0.0
                } else {
                    (audio.squared / count as f64).sqrt() as f32
                };
                audio.squared = 0.0;
                audio.count = 0;
                (rms, count)
            };
            let _ = app.emit("voice://level", serde_json::json!({"rms": rms}));
            if silence.update(rms, count, opts.silence_secs) {
                stopped.store(true, Ordering::Release);
                drop(stream);
                let _ = app.emit("voice://stopped", serde_json::json!({"reason": "silence"}));
                if let Some(worker) = decoding {
                    let _ = worker.join();
                }
                return;
            }
            if last_decode.elapsed() >= Duration::from_millis(1500)
                && decoding.as_ref().is_none_or(|d| d.is_finished())
            {
                if let Some(worker) = decoding.take() {
                    let _ = worker.join();
                }
                last_decode = Instant::now();
                let (core, app, stopped) = (core.clone(), app.clone(), stopped.clone());
                let buffer = samples.lock().unwrap().samples.clone();
                // ponytail: re-decodes the whole buffer for the live preview, O(n^2) over a long dictation; switch to a streaming model or a VAD-chunked decode if minutes-long dictation matters.
                decoding = Some(std::thread::spawn(move || {
                    let mut loaded = core.loaded.lock().unwrap();
                    if let Some(loaded) = loaded.as_mut() {
                        let result = loaded.recognizer.decode(&buffer);
                        loaded.used = Instant::now();
                        if !stopped.load(Ordering::Acquire) {
                            if let Ok(text) = result {
                                let _ =
                                    app.emit("voice://partial", serde_json::json!({"text": text}));
                            }
                        }
                    }
                }));
            }
        }
        drop(stream);
        if let Some(worker) = decoding {
            let _ = worker.join();
        }
    });
    match ready_rx.recv().map_err(|_| MIC_ERROR.to_string())? {
        Ok(()) => Ok(Recording {
            model,
            audio,
            stop,
            thread: Some(thread),
        }),
        Err(e) => {
            let _ = thread.join();
            Err(e)
        }
    }
}

#[tauri::command]
pub async fn voice_start(
    app: tauri::AppHandle,
    voice: tauri::State<'_, Voice>,
    opts: VoiceOptions,
) -> Result<(), String> {
    models::model(&opts.model)?;
    if ![0, 30, 60, 120].contains(&opts.silence_secs)
        || opts.language.contains('\0')
        || opts.input.contains('\0')
    {
        return Err("Choose dictation options in Settings › Voice.".into());
    }
    let core = voice.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut recording = core.recording.lock().unwrap();
        if recording.is_some() {
            return Err("Dictation is already listening.".into());
        }
        {
            let mut loaded = core.loaded.lock().unwrap();
            if !loaded.as_ref().is_some_and(|l| {
                l.model == opts.model && l.language == opts.language && l.gpu == opts.gpu
            }) {
                *loaded = Some(models::load(&opts)?);
            }
            loaded.as_mut().unwrap().used = Instant::now();
        }
        *recording = Some(capture(core.clone(), app, opts)?);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn finish(core: &Core, decode: bool) -> Result<String, String> {
    let mut recording = core.recording.lock().unwrap();
    let Some(mut current) = recording.take() else {
        return Ok(String::new());
    };
    current.close();
    let mut loaded = core.loaded.lock().unwrap();
    let result = if decode {
        let audio = current.audio.lock().unwrap();
        loaded
            .as_ref()
            .ok_or("The speech model was unloaded. Try dictating again.")?
            .recognizer
            .decode(&audio.samples)
    } else {
        Ok(String::new())
    };
    if let Some(l) = loaded.as_mut() {
        l.used = Instant::now();
    }
    if core.keep_loaded.load(Ordering::Relaxed) == 0 {
        *loaded = None;
    }
    result
}

#[tauri::command]
pub async fn voice_stop(voice: tauri::State<'_, Voice>) -> Result<String, String> {
    let core = voice.0.clone();
    tauri::async_runtime::spawn_blocking(move || finish(&core, true))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn voice_cancel(voice: tauri::State<'_, Voice>) -> Result<(), String> {
    let core = voice.0.clone();
    tauri::async_runtime::spawn_blocking(move || finish(&core, false).map(|_| ()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn voice_unload_after(voice: tauri::State<'_, Voice>, secs: i64) -> Result<(), String> {
    if ![0, 300, 900, -1].contains(&secs) {
        return Err("Choose how long to keep the speech model loaded in Settings › Voice.".into());
    }
    voice.0.keep_loaded.store(secs, Ordering::Relaxed);
    if secs == 0 {
        let core = voice.0.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let recording = core.recording.lock().unwrap();
            if recording.is_none() {
                *core.loaded.lock().unwrap() = None;
            }
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resamples_to_16khz_across_callbacks() {
        for rate in [8000, 16000, 44100, 48000] {
            let input = (0..rate)
                .map(|i| (i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin())
                .collect::<Vec<_>>();
            let mut whole = Audio::default();
            Resampler::new(rate).append(input.iter().copied(), &mut whole);
            let mut chunks = Audio::default();
            let mut resampler = Resampler::new(rate);
            for chunk in input.chunks(137) {
                resampler.append(chunk.iter().copied(), &mut chunks);
            }
            assert!(whole.samples.len().abs_diff(16000) <= 2);
            assert_eq!(whole.samples.len(), chunks.samples.len());
            assert!(whole
                .samples
                .iter()
                .zip(&chunks.samples)
                .all(|(a, b)| (a - b).abs() < 0.001));
            assert!(
                (whole.samples[9] - (9.0_f32 / 16000.0 * 440.0 * std::f32::consts::TAU).sin())
                    .abs()
                    < 0.02
            );
        }
    }
    #[test]
    fn silence_resets_on_speech_and_zero_never_stops() {
        let mut detector = Silence::default();
        assert!(!detector.update(0.0, 16000 * 29, 30));
        assert!(!detector.update(0.1, 16000, 30));
        assert!(!detector.update(0.0, 16000 * 29, 30));
        assert!(detector.update(0.0, 16000, 30));
        assert!(!detector.update(0.0, 16000 * 120, 0));
    }
}
