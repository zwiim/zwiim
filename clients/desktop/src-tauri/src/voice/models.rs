//! The local model catalog, installation and recognizer configuration.

use super::{Loaded, Voice, VoiceOptions};
use serde::Serialize;
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OnlineRecognizer, OnlineRecognizerConfig,
};
use std::path::{Path, PathBuf};
use tauri::Emitter;

#[derive(Clone, Serialize)]
pub struct ModelInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub quality: &'static str,
    pub recommended: bool,
    pub size_mb: f64,
    pub installed: bool,
    /// A download of it is running now, possibly started by a settings view since closed.
    pub downloading: bool,
    archive: &'static str,
}

// Archive sizes verified against the release assets, in decimal MB as on the Mac.
const CATALOG: &[ModelInfo] = &[
    ModelInfo {
        id: "parakeet-tdt-v3",
        label: "Parakeet TDT v3",
        description: "Fast. 25 European languages.",
        quality: "Very good",
        recommended: true,
        size_mb: 487.211443,
        installed: false,
        downloading: false,
        archive: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
    },
    ModelInfo {
        id: "parakeet-tdt-v2",
        label: "Parakeet TDT v2",
        description: "Fast. English only, and the most accurate at it.",
        quality: "Excellent",
        recommended: false,
        size_mb: 482.440461,
        installed: false,
        downloading: false,
        archive: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8",
    },
    ModelInfo {
        id: "whisper-tiny",
        label: "Whisper Tiny",
        description: "Smallest download. Good for trying dictation out.",
        quality: "Fair",
        recommended: false,
        size_mb: 116.204861,
        installed: false,
        downloading: false,
        archive: "sherpa-onnx-whisper-tiny",
    },
    ModelInfo {
        id: "whisper-turbo",
        label: "Whisper Large v3 Turbo",
        description: "Most languages, about 100. Slow without a GPU.",
        quality: "Very good",
        recommended: false,
        size_mb: 563.790207,
        installed: false,
        downloading: false,
        archive: "sherpa-onnx-whisper-turbo",
    },
    ModelInfo {
        id: "sense-voice",
        label: "SenseVoice",
        description: "Fast. For Chinese, Japanese, Korean and Cantonese.",
        quality: "Good",
        recommended: false,
        size_mb: 163.002883,
        installed: false,
        downloading: false,
        archive: "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17",
    },
    ModelInfo {
        id: "nemotron",
        label: "Nemotron streaming",
        description: "Fast. Streaming speech recognition in English.",
        quality: "",
        recommended: false,
        size_mb: 468.994262,
        installed: false,
        downloading: false,
        archive: "sherpa-onnx-nemotron-speech-streaming-en-0.6b-int8-2026-01-14",
    },
];

pub(super) fn model(id: &str) -> Result<&'static ModelInfo, String> {
    CATALOG
        .iter()
        .find(|m| m.id == id)
        .ok_or_else(|| "Choose a speech model from Settings › Voice.".into())
}

pub(super) fn models_dir() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|d| d.join("com.zwiim.app/speech-models"))
        .ok_or_else(|| "Zwiim can't find its app data folder.".into())
}

fn model_path(root: &Path, id: &str) -> Result<PathBuf, String> {
    Ok(root.join(model(id)?.id))
}

#[tauri::command]
pub fn voice_models(voice: tauri::State<'_, Voice>) -> Result<Vec<ModelInfo>, String> {
    let root = models_dir()?;
    let downloads = voice.0.downloads.lock().unwrap();
    Ok(CATALOG
        .iter()
        .cloned()
        .map(|mut m| {
            m.installed = root.join(m.id).is_dir();
            m.downloading = downloads.contains(m.id);
            m
        })
        .collect())
}

fn find_file(root: &Path, prefix: &str, suffix: &str) -> Result<String, String> {
    let mut paths = std::fs::read_dir(root)
        .map_err(|_| "The speech model is missing. Download it again in Settings › Voice.")?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(prefix) && n.ends_with(suffix))
        })
        .collect::<Vec<_>>();
    // Prefer the quantized model when an archive also includes full precision files.
    paths.sort_by_key(|p| (!p.to_string_lossy().contains("int8"), p.clone()));
    paths.first().map(|p| p.to_string_lossy().into_owned()).ok_or_else(|| format!("The speech model has no {prefix}{suffix} file. Download it again in Settings › Voice."))
}

pub(super) enum Recognizer {
    Offline(OfflineRecognizer),
    Online(OnlineRecognizer),
}

impl Recognizer {
    pub(super) fn decode(&self, samples: &[f32]) -> Result<String, String> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let text = match self {
            Self::Offline(r) => {
                let stream = r.create_stream();
                stream.accept_waveform(16000, samples);
                r.decode(&stream);
                stream.get_result().map(|r| r.text)
            }
            Self::Online(r) => {
                let stream = r.create_stream();
                stream.accept_waveform(16000, samples);
                stream.accept_waveform(16000, &vec![0.0; 8000]);
                stream.input_finished();
                while r.is_ready(&stream) {
                    r.decode(&stream);
                }
                r.get_result(&stream).map(|r| r.text)
            }
        };
        text.ok_or_else(|| "Zwiim couldn't transcribe that recording. Try again.".into())
    }
}

pub(super) fn load(opts: &VoiceOptions) -> Result<Loaded, String> {
    let m = model(&opts.model)?;
    let root = model_path(&models_dir()?, m.id)?;
    if !root.is_dir() {
        return Err(format!("Download {} in Settings › Voice first.", m.label));
    }
    let file = |prefix: &str, suffix: &str| find_file(&root, prefix, suffix).map(Some);
    let provider = Some(if opts.gpu { "coreml" } else { "cpu" }.into());
    let recognizer = if m.id == "nemotron" {
        let mut c = OnlineRecognizerConfig::default();
        c.model_config.transducer.encoder = file("encoder", ".onnx")?;
        c.model_config.transducer.decoder = file("decoder", ".onnx")?;
        c.model_config.transducer.joiner = file("joiner", ".onnx")?;
        c.model_config.tokens = file("", "tokens.txt")?;
        c.model_config.model_type = Some("nemo_transducer".into());
        c.model_config.provider = provider;
        c.model_config.num_threads = 2;
        OnlineRecognizer::create(&c).map(Recognizer::Online)
    } else {
        let mut c = OfflineRecognizerConfig::default();
        c.model_config.tokens = file("", "tokens.txt")?;
        c.model_config.provider = provider;
        c.model_config.num_threads = 2;
        if m.id.starts_with("parakeet") {
            c.model_config.transducer.encoder = file("encoder", ".onnx")?;
            c.model_config.transducer.decoder = file("decoder", ".onnx")?;
            c.model_config.transducer.joiner = file("joiner", ".onnx")?;
            c.model_config.model_type = Some("nemo_transducer".into());
        } else if m.id.starts_with("whisper") {
            // Whisper names its files tiny-encoder.int8.onnx, turbo-decoder.int8.onnx, etc.
            let prefix = if m.id == "whisper-tiny" {
                "tiny"
            } else {
                "turbo"
            };
            c.model_config.whisper.encoder = file(&format!("{prefix}-encoder"), ".onnx")?;
            c.model_config.whisper.decoder = file(&format!("{prefix}-decoder"), ".onnx")?;
            c.model_config.whisper.language = Some(
                if opts.language == "auto" {
                    ""
                } else {
                    &opts.language
                }
                .into(),
            );
            c.model_config.whisper.task = Some("transcribe".into());
        } else {
            c.model_config.sense_voice.model = file("model", ".onnx")?;
            c.model_config.sense_voice.language = Some(opts.language.clone());
            c.model_config.sense_voice.use_itn = true;
        }
        OfflineRecognizer::create(&c).map(Recognizer::Offline)
    }
    .ok_or("Zwiim couldn't load the speech model. Try turning off Use GPU in Settings › Voice.")?;
    Ok(Loaded {
        model: opts.model.clone(),
        language: opts.language.clone(),
        gpu: opts.gpu,
        recognizer,
        used: std::time::Instant::now(),
    })
}

async fn download(app: &tauri::AppHandle, m: &ModelInfo) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let root = models_dir()?;
    let dest = model_path(&root, m.id)?;
    if dest.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(&root)
        .map_err(|e| format!("Zwiim can't create its speech models folder: {e}"))?;
    let staging = root.join(format!(".{}-{:016x}", m.id, rand::random::<u64>()));
    std::fs::create_dir(&staging).map_err(|e| format!("Zwiim can't save the speech model: {e}"))?;
    let result = async {
        let archive = staging.join("model.tar.bz2");
        let url = format!(
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/{}.tar.bz2",
            m.archive
        );
        let mut response = reqwest::Client::new()
            .get(url)
            .send()
            .await
            .map_err(|e| format!("The speech model download failed: {e}"))?
            .error_for_status()
            .map_err(|e| format!("The speech model download failed: {e}"))?;
        let total = response.content_length();
        let mut file = tokio::fs::File::create(&archive)
            .await
            .map_err(|e| e.to_string())?;
        let mut downloaded = 0u64;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("The speech model download stopped: {e}"))?
        {
            file.write_all(&chunk)
                .await
                .map_err(|e| format!("Zwiim can't save the speech model: {e}"))?;
            downloaded += chunk.len() as u64;
            let _ = app.emit(
                "voice://download",
                serde_json::json!({"id": m.id, "downloaded": downloaded, "total": total}),
            );
        }
        file.sync_all().await.map_err(|e| e.to_string())?;
        drop(file);
        let folder = staging.clone();
        let name = m.archive.to_string();
        tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
            let reader = bzip2::read::BzDecoder::new(
                std::fs::File::open(archive).map_err(|e| e.to_string())?,
            );
            let mut tar = tar::Archive::new(reader);
            for entry in tar.entries().map_err(|e| e.to_string())? {
                let mut entry = entry.map_err(|e| e.to_string())?;
                let kind = entry.header().entry_type();
                if !(kind.is_file() || kind.is_dir())
                    || !entry.unpack_in(&folder).map_err(|e| e.to_string())?
                {
                    return Err("The speech model archive contains an unsafe path.".into());
                }
            }
            let extracted = folder.join(name);
            find_file(&extracted, "", "tokens.txt")?;
            std::fs::rename(extracted, dest)
                .map_err(|e| format!("Zwiim can't install the speech model: {e}"))
        })
        .await
        .map_err(|e| e.to_string())?
    }
    .await;
    let _ = std::fs::remove_dir_all(staging);
    result
}

#[tauri::command]
pub async fn voice_model_download(
    app: tauri::AppHandle,
    voice: tauri::State<'_, Voice>,
    id: String,
) -> Result<(), String> {
    let m = model(&id)?;
    if !voice.0.downloads.lock().unwrap().insert(id.clone()) {
        return Err("That speech model is already downloading.".into());
    }
    let result = download(&app, m).await;
    voice.0.downloads.lock().unwrap().remove(&id);
    let event = match &result {
        Ok(()) => serde_json::json!({"id": id, "done": true}),
        Err(e) => serde_json::json!({"id": id, "error": e}),
    };
    let _ = app.emit("voice://download", event);
    result
}

#[tauri::command]
pub async fn voice_model_delete(voice: tauri::State<'_, Voice>, id: String) -> Result<(), String> {
    let path = model_path(&models_dir()?, &id)?;
    let core = voice.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let downloads = core.downloads.lock().unwrap();
        if downloads.contains(&id) {
            return Err("Wait for the speech model download to finish.".into());
        }
        let recording = core.recording.lock().unwrap();
        if recording.as_ref().is_some_and(|r| r.model == id) {
            return Err("Stop dictation before deleting its speech model.".into());
        }
        let mut loaded = core.loaded.lock().unwrap();
        if loaded.as_ref().is_some_and(|l| l.model == id) {
            *loaded = None;
        }
        match std::fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Zwiim can't delete the speech model: {e}")),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_ids_and_recommendation() {
        let ids = CATALOG
            .iter()
            .map(|m| m.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), CATALOG.len());
        assert_eq!(
            CATALOG
                .iter()
                .filter(|m| m.recommended)
                .map(|m| m.id)
                .collect::<Vec<_>>(),
            ["parakeet-tdt-v3"]
        );
    }
    #[test]
    fn finds_whisper_tokens_and_prefers_quantized_models() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "tiny-tokens.txt",
            "tiny-encoder.onnx",
            "tiny-encoder.int8.onnx",
        ] {
            std::fs::write(dir.path().join(name), b"test").unwrap();
        }
        assert!(find_file(dir.path(), "", "tokens.txt")
            .unwrap()
            .ends_with("tiny-tokens.txt"));
        assert!(find_file(dir.path(), "tiny-encoder", ".onnx")
            .unwrap()
            .ends_with("tiny-encoder.int8.onnx"));
        assert!(find_file(dir.path(), "joiner", ".onnx").is_err());
    }

    #[test]
    fn delete_refuses_unknown_and_traversal() {
        let root = Path::new("/speech-models");
        for id in [
            "missing",
            "../parakeet-tdt-v3",
            "/tmp",
            "..",
            "parakeet-tdt-v3/../../tmp",
        ] {
            assert!(model_path(root, id).is_err());
        }
        assert_eq!(
            model_path(root, "whisper-tiny").unwrap(),
            root.join("whisper-tiny")
        );
    }
}
