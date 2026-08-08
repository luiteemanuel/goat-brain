use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const WHISPER_PORT: u16 = 8081;
const INFERENCE_URL: &str = "http://127.0.0.1:8081/inference";
const MODEL_FILE: &str = "ggml-large-v3-turbo.bin";
const SR: u32 = 16000;

pub const MODEL_NAME: &str = "ggml-large-v3-turbo.bin";

pub struct WhisperBackend {
    proc: Mutex<Option<Child>>,
    client: reqwest::blocking::Client,
}

impl WhisperBackend {
    pub fn new() -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("falha ao criar http client");
        Self {
            proc: Mutex::new(None),
            client,
        }
    }

    pub fn is_online(&self) -> bool {
        self.client
            .get(INFERENCE_URL)
            .timeout(Duration::from_secs(1))
            .send()
            .is_ok_and(|response| response.status().is_success())
    }

    pub fn start(&self, resource_dir: &Path) -> anyhow::Result<()> {
        if self.is_online() {
            return Ok(());
        }
        let bin = resolve_path(resource_dir, "whisper-server", "../bin/whisper-server");
        let model = resolve_path(
            resource_dir,
            MODEL_FILE,
            "../models/ggml-large-v3-turbo.bin",
        );
        let child = Command::new(&bin)
            .args([
                "-m",
                model.to_str().unwrap_or(MODEL_FILE),
                "-l",
                "pt",
                "-nt",
                "--port",
                &WHISPER_PORT.to_string(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("falha ao iniciar {}: {e}", bin.display()))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if self.is_online() {
                *self.proc.lock().unwrap() = Some(child);
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let mut child = child;
        let _ = child.kill();
        let _ = child.wait();
        anyhow::bail!("whisper-server não subiu em 30s")
    }

    pub fn transcribe(&self, pcm: &[i16], language: &str) -> anyhow::Result<String> {
        let wav = pcm_to_wav(pcm);
        let form = reqwest::blocking::multipart::Form::new()
            .part(
                "file",
                reqwest::blocking::multipart::Part::bytes(wav)
                    .file_name("seg.wav")
                    .mime_str("audio/wav")?,
            )
            .text("response_format", "json")
            .text("language", language.to_string());
        let resp = self.client.post(INFERENCE_URL).multipart(form).send()?;
        let json: serde_json::Value = resp.error_for_status()?.json()?;
        Ok(json
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string())
    }

    pub fn stop(&self) {
        if let Some(mut child) = self.proc.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn resolve_path(resource_dir: &Path, name: &str, dev_rel: &str) -> PathBuf {
    let override_name = if name == MODEL_FILE {
        "GOAT_WHISPER_MODEL"
    } else {
        "GOAT_WHISPER_BIN"
    };
    if let Ok(path) = std::env::var(override_name) {
        return PathBuf::from(path);
    }

    let candidates = [
        resource_dir.join(name),
        resource_dir.join("bin").join(name),
        resource_dir.join("models").join(name),
        PathBuf::from("/usr/lib/goat-reuniao").join(dev_rel.trim_start_matches("../")),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(dev_rel),
    ];
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .unwrap_or_else(|| resource_dir.join(name))
}

fn pcm_to_wav(pcm: &[i16]) -> Vec<u8> {
    let n = pcm.len() * 2;
    let mut w = Vec::with_capacity(44 + n);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + n as u32).to_le_bytes());
    w.extend_from_slice(b"WAVE");
    w.extend_from_slice(b"fmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&SR.to_le_bytes());
    w.extend_from_slice(&(SR * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(n as u32).to_le_bytes());
    for s in pcm {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_to_wav_has_valid_headers_and_payload() {
        let wav = pcm_to_wav(&[0, 1, -2]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
        assert_eq!(wav.len(), 50);
    }

    #[test]
    fn resolve_path_prefers_resource_layout() {
        let root = std::env::temp_dir().join(format!("goat-whisper-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let path = root.join("bin/whisper-server");
        std::fs::write(&path, b"test").unwrap();
        assert_eq!(
            resolve_path(&root, "whisper-server", "../bin/whisper-server"),
            path
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
