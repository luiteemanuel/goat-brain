use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::whisper::WhisperBackend;

const SEG_SEC: usize = 30;
const SEG_SAMPLES: usize = SEG_SEC * 16000;

#[derive(Clone, Serialize)]
pub struct VideoSegmentEvent {
    pub index: usize,
    pub ts: String,
    pub text: String,
}

#[derive(Clone, Serialize)]
pub struct VideoProgressEvent {
    pub done: usize,
    pub total: usize,
}

#[derive(Clone, Serialize)]
pub struct VideoDoneEvent {
    pub segments: usize,
    pub text: String,
}

#[derive(Clone, Serialize)]
pub struct VideoErrorEvent {
    pub message: String,
}

pub struct VideoJob {
    cancel: AtomicBool,
    running: AtomicBool,
    state_lock: Mutex<()>,
}

impl VideoJob {
    pub fn new() -> Self {
        Self {
            cancel: AtomicBool::new(false),
            running: AtomicBool::new(false),
            state_lock: Mutex::new(()),
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub fn try_start(&self) -> bool {
        let _guard = self.state_lock.lock().unwrap();
        if self.running.load(Ordering::Relaxed) {
            return false;
        }
        self.cancel.store(false, Ordering::Relaxed);
        self.running.store(true, Ordering::Release);
        true
    }

    fn finish(&self) {
        let _guard = self.state_lock.lock().unwrap();
        self.running.store(false, Ordering::Release);
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }

    fn is_canceled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}

pub fn start(
    app: AppHandle,
    backend: Arc<WhisperBackend>,
    job: Arc<VideoJob>,
    path: String,
    language: String,
) -> bool {
    if !job.try_start() {
        return false;
    }
    std::thread::spawn(move || run_job(app, backend, job, path, language));
    true
}

fn run_job(
    app: AppHandle,
    backend: Arc<WhisperBackend>,
    job: Arc<VideoJob>,
    path: String,
    language: String,
) {
    let result = transcribe_video(&app, &backend, &job, &path, &language);
    job.finish();
    if let Err(e) = result {
        let _ = app.emit(
            "video_error",
            VideoErrorEvent {
                message: e.to_string(),
            },
        );
    }
}

fn probe_duration_secs(path: &str) -> Option<f64> {
    let out = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim().parse::<f64>().ok()
}

fn transcribe_video(
    app: &AppHandle,
    backend: &WhisperBackend,
    job: &VideoJob,
    path: &str,
    language: &str,
) -> anyhow::Result<()> {
    if !Path::new(path).exists() {
        bail!("arquivo não encontrado: {path}");
    }
    if !backend.is_online() {
        bail!("motor whisper offline — aguarde o app carregar ou reinicie");
    }
    let total_segments = probe_duration_secs(path)
        .map(|d| ((d.ceil() as usize) + SEG_SEC - 1) / SEG_SEC)
        .unwrap_or(0);
    let mut child = Command::new("ffmpeg")
        .args([
            "-v", "error", "-i", path, "-ar", "16000", "-ac", "1", "-f", "s16le", "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("ffmpeg indisponível: {e} — instale com 'sudo pacman -S ffmpeg'"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("falha ao ler áudio do ffmpeg"))?;
    let mut pending = Vec::with_capacity(SEG_SAMPLES * 2);
    let mut read_buf = [0u8; 64 * 1024];
    let mut texts = Vec::new();
    let mut index = 0usize;

    loop {
        if job.is_canceled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = app.emit("video_canceled", ());
            return Ok(());
        }
        let count = stdout.read(&mut read_buf)?;
        if count == 0 {
            break;
        }
        pending.extend_from_slice(&read_buf[..count]);
        while pending.len() >= SEG_SAMPLES * 2 {
            let bytes: Vec<u8> = pending.drain(..SEG_SAMPLES * 2).collect();
            transcribe_chunk(app, backend, job, language, &bytes, index, total_segments, &mut texts)?;
            index += 1;
        }
    }
    if !pending.is_empty() {
        transcribe_chunk(app, backend, job, language, &pending, index, total_segments, &mut texts)?;
    }
    let status = child.wait()?;
    if !status.success() {
        bail!("ffmpeg falhou ao extrair o áudio do arquivo");
    }
    if job.is_canceled() {
        let _ = app.emit("video_canceled", ());
        return Ok(());
    }
    let _ = app.emit(
        "video_done",
        VideoDoneEvent {
            segments: texts.len(),
            text: texts.join("\n\n"),
        },
    );
    Ok(())
}

fn transcribe_chunk(
    app: &AppHandle,
    backend: &WhisperBackend,
    job: &VideoJob,
    language: &str,
    bytes: &[u8],
    index: usize,
    total: usize,
    texts: &mut Vec<String>,
) -> anyhow::Result<()> {
    if job.is_canceled() {
        return Ok(());
    }
    let samples = bytes_to_i16(bytes);
    if samples.is_empty() {
        return Ok(());
    }
    let text = backend
        .transcribe(&samples, language)
        .map_err(|e| anyhow!("falha na transcrição (segmento {}): {e}", index + 1))?;
    let ts = fmt_ts(index * SEG_SEC);
    texts.push(text.clone());
    let _ = app.emit(
        "video_segment",
        VideoSegmentEvent {
            index: index + 1,
            ts,
            text,
        },
    );
    let _ = app.emit(
        "video_progress",
        VideoProgressEvent {
            done: index + 1,
            total,
        },
    );
    Ok(())
}

fn bytes_to_i16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect()
}

fn fmt_ts(secs: usize) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_job_rejects_concurrent_start_and_allows_restart() {
        let job = VideoJob::new();
        assert!(job.try_start());
        assert!(job.is_running());
        assert!(!job.try_start());
        job.finish();
        assert!(!job.is_running());
        assert!(job.try_start());
    }

    #[test]
    fn video_job_cancel_is_observable() {
        let job = VideoJob::new();
        assert!(!job.is_canceled());
        job.cancel();
        assert!(job.is_canceled());
    }

    #[test]
    fn formats_timestamps() {
        assert_eq!(fmt_ts(3723), "01:02:03");
    }

    #[test]
    fn converts_little_endian_pcm_and_ignores_partial_byte() {
        assert_eq!(bytes_to_i16(&[0, 0, 0xff, 0x7f, 0x80]), vec![0, 32767]);
    }

    #[test]
    fn chunks_are_bounded_to_thirty_seconds() {
        let bytes = vec![0u8; SEG_SAMPLES * 2 + 2];
        let mut chunks = bytes.chunks_exact(SEG_SAMPLES * 2);
        assert_eq!(chunks.next().unwrap().len(), SEG_SAMPLES * 2);
        assert_eq!(chunks.remainder().len(), 2);
    }
}
