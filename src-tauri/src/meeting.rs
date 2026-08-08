use std::io::Read;
use std::ops::Deref;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use webrtc_vad::{SampleRate, Vad, VadMode};

use cpal::traits::DeviceTrait;

use crate::audio;
use crate::whisper::WhisperBackend;

struct SafeVad(Mutex<Vad>);

unsafe impl Send for SafeVad {}
unsafe impl Sync for SafeVad {}

impl Deref for SafeVad {
    type Target = Mutex<Vad>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

const HANGOVER_FRAMES: usize = 40;
const MAX_SEGMENT_SAMPLES: usize = 30 * audio::SR as usize;
const MIN_SEGMENT_SAMPLES: usize = 400;

#[derive(Clone, Serialize)]
pub struct SegmentEvent {
    pub id: u32,
    pub text: String,
    pub ts: String,
}

#[derive(Clone, Serialize)]
pub struct QueuedEvent {
    pub id: u32,
}

#[derive(Clone, Serialize)]
pub struct StateEvent {
    pub state: &'static str,
}

#[derive(Clone, Serialize)]
pub struct ErrorEvent {
    pub message: String,
}

#[derive(Clone, Serialize)]
pub struct TtResultEvent {
    pub text: String,
}

struct Segment {
    id: u32,
    pcm: Vec<i16>,
    ts: String,
}

pub struct MeetingRecorder {
    _backend: Arc<WhisperBackend>,
    app: AppHandle,
    _vad: Arc<SafeVad>,
    running: Arc<AtomicBool>,
    _seg_id: Arc<AtomicU32>,
    audio_tx: Sender<Vec<i16>>,
    monitor_tx: Sender<Vec<i16>>,
    _seg_tx: Sender<Segment>,
    mic_stream: Option<cpal::Stream>,
    monitor_stream: Option<cpal::Stream>,
    monitor_proc: Option<Child>,
}

impl MeetingRecorder {
    pub fn new(backend: Arc<WhisperBackend>, app: AppHandle) -> Self {
        let (audio_tx, audio_rx) = mpsc::channel();
        let (monitor_tx, monitor_rx) = mpsc::channel();
        let (seg_tx, seg_rx) = mpsc::channel();
        let vad = Arc::new(SafeVad(Mutex::new(Vad::new_with_rate_and_mode(
            SampleRate::Rate16kHz,
            VadMode::VeryAggressive,
        ))));
        let running = Arc::new(AtomicBool::new(false));
        let seg_id = Arc::new(AtomicU32::new(0));

        let app_vad = vad.clone();
        let app_running = running.clone();
        let app_seg = seg_id.clone();
        let app_emit = app.clone();
        let seg_tx_loop = seg_tx.clone();
        std::thread::spawn(move || {
            vad_loop(
                audio_rx,
                monitor_rx,
                seg_tx_loop,
                app_vad,
                app_emit,
                app_running,
                app_seg,
            )
        });

        let backend_loop = backend.clone();
        let app_emit = app.clone();
        std::thread::spawn(move || transcribe_loop(seg_rx, backend_loop, app_emit));

        Self {
            _backend: backend,
            app,
            _vad: vad,
            running,
            _seg_id: seg_id,
            audio_tx,
            monitor_tx,
            _seg_tx: seg_tx,
            mic_stream: None,
            monitor_stream: None,
            monitor_proc: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn start(&mut self) -> anyhow::Result<()> {
        if self.is_running() {
            return Ok(());
        }
        self.running.store(true, Ordering::Relaxed);

        crate::audio::list_input_devices();
        let tx = self.audio_tx.clone();
        match audio::open_mic_stream(move |chunk| {
            let _ = tx.send(chunk);
        }) {
            Ok(stream) => self.mic_stream = Some(stream),
            Err(error) => {
                self.running.store(false, Ordering::Relaxed);
                let _ = self.app.emit("meeting_state", StateEvent { state: "off" });
                return Err(error);
            }
        }

        if let Some(dev) = audio::find_monitor_device() {
            if let Ok(desc) = dev.description() {
                eprintln!("[goat] monitor via cpal: {}", desc.name());
            }
            let tx = self.monitor_tx.clone();
            match audio::open_device_stream(&dev, move |chunk| {
                let _ = tx.send(chunk);
            }) {
                Ok(stream) => self.monitor_stream = Some(stream),
                Err(error) => {
                    self.stop();
                    return Err(error);
                }
            }
        } else if let Some(child) =
            spawn_parec_monitor(self.monitor_tx.clone(), self.running.clone())
        {
            eprintln!(
                "[goat] monitor via parec (source: {})",
                get_monitor_source().unwrap_or_default()
            );
            self.monitor_proc = Some(child);
        } else {
            let error = anyhow::anyhow!("nenhum dispositivo de monitor disponível");
            self.stop();
            return Err(error);
        }

        let _ = self.app.emit("meeting_state", StateEvent { state: "on" });
        Ok(())
    }

    pub fn stop(&mut self) {
        if !self.is_running() {
            return;
        }
        self.running.store(false, Ordering::Relaxed);
        let _ = self.audio_tx.send(Vec::new());
        self.mic_stream = None;
        self.monitor_stream = None;
        if let Some(mut child) = self.monitor_proc.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = self.app.emit("meeting_state", StateEvent { state: "off" });
    }
}

fn vad_loop(
    audio_rx: mpsc::Receiver<Vec<i16>>,
    monitor_rx: mpsc::Receiver<Vec<i16>>,
    seg_tx: Sender<Segment>,
    vad: Arc<SafeVad>,
    app: AppHandle,
    running: Arc<AtomicBool>,
    seg_id: Arc<AtomicU32>,
) {
    let mut buffer: Vec<i16> = Vec::new();
    let mut start_ts = SystemTime::now();
    let mut speaking = false;
    let mut hangover = 0usize;
    let mut chunks_seen = 0usize;
    let mut last_log = Instant::now();
    let mut speech_frames = 0usize;
    let mut total_frames = 0usize;
    let mut monitor_pending = Vec::new();
    let mut vad_pending = Vec::new();
    let flush = |buffer: &mut Vec<i16>,
                 start: SystemTime,
                 seg_tx: &Sender<Segment>,
                 app: &AppHandle,
                 seg_id: &AtomicU32| {
        if buffer.len() < MIN_SEGMENT_SAMPLES {
            buffer.clear();
            return;
        }
        let id = seg_id.fetch_add(1, Ordering::Relaxed) + 1;
        let ts = chrono::DateTime::<chrono::Local>::from(start)
            .format("%H:%M:%S")
            .to_string();
        let _ = app.emit("segment_queued", QueuedEvent { id });
        let _ = seg_tx.send(Segment {
            id,
            pcm: std::mem::take(buffer),
            ts,
        });
    };

    loop {
        match audio_rx.recv_timeout(Duration::from_millis(300)) {
            Ok(mic_chunk) => {
                if !running.load(Ordering::Relaxed) {
                    if speaking && !buffer.is_empty() {
                        flush(&mut buffer, start_ts, &seg_tx, &app, &seg_id);
                    }
                    break;
                }
                if mic_chunk.is_empty() {
                    continue;
                }
                let mut chunk = mic_chunk;
                while let Ok(monitor_chunk) = monitor_rx.try_recv() {
                    monitor_pending.extend(monitor_chunk);
                }
                chunk = mix_with_pending(&chunk, &mut monitor_pending);
                vad_pending.extend_from_slice(&chunk);
                let mut is_speech = false;
                {
                    let mut vad_guard = vad.lock().unwrap();
                    while vad_pending.len() >= audio::BLOCK {
                        let frame: Vec<i16> = vad_pending.drain(..audio::BLOCK).collect();
                        if let Ok(v) = vad_guard.is_voice_segment(&frame) {
                            total_frames += 1;
                            if v {
                                is_speech = true;
                                speech_frames += 1;
                            }
                        }
                    }
                }
                chunks_seen += 1;
                if last_log.elapsed() >= Duration::from_secs(5) {
                    eprintln!(
                        "[goat] áudio: chunks={} voz={}/{} falando={}",
                        chunks_seen, speech_frames, total_frames, speaking
                    );
                    last_log = Instant::now();
                }
                if is_speech {
                    if !speaking {
                        speaking = true;
                        start_ts = SystemTime::now();
                        buffer.clear();
                    }
                    buffer.extend_from_slice(&chunk);
                    hangover = 0;
                } else if speaking {
                    buffer.extend_from_slice(&chunk);
                    hangover += 1;
                    if hangover >= HANGOVER_FRAMES {
                        flush(&mut buffer, start_ts, &seg_tx, &app, &seg_id);
                        speaking = false;
                        hangover = 0;
                    }
                }
                if speaking && buffer.len() >= MAX_SEGMENT_SAMPLES {
                    flush(&mut buffer, start_ts, &seg_tx, &app, &seg_id);
                    start_ts = SystemTime::now();
                    speaking = false;
                    hangover = 0;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if speaking && !buffer.is_empty() {
                    flush(&mut buffer, start_ts, &seg_tx, &app, &seg_id);
                }
                buffer.clear();
                speaking = false;
                hangover = 0;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn transcribe_loop(seg_rx: mpsc::Receiver<Segment>, backend: Arc<WhisperBackend>, app: AppHandle) {
    while let Ok(seg) = seg_rx.recv() {
        let started = Instant::now();
        let text = match backend.transcribe(&seg.pcm, "pt") {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[goat] erro transcrição seg {}: {e}", seg.id);
                let _ = app.emit(
                    "error",
                    ErrorEvent {
                        message: format!("falha na transcrição: {e}"),
                    },
                );
                continue;
            }
        };
        eprintln!(
            "[goat] seg {} em {:.1}s ({} amostras): {:?}",
            seg.id,
            started.elapsed().as_secs_f32(),
            seg.pcm.len(),
            text.chars().take(80).collect::<String>()
        );
        if !text.is_empty() {
            let _ = app.emit(
                "segment",
                SegmentEvent {
                    id: seg.id,
                    text,
                    ts: seg.ts,
                },
            );
        }
    }
}

fn mix_with_pending(input: &[i16], pending: &mut Vec<i16>) -> Vec<i16> {
    let n = input.len().max(pending.len());
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let value = input.get(i).copied().unwrap_or_default() as i32
            + pending.get(i).copied().unwrap_or_default() as i32;
        out.push(value.clamp(i16::MIN as i32, i16::MAX as i32) as i16);
    }
    if pending.len() > input.len() {
        pending.drain(..input.len());
    } else {
        pending.clear();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::mix_with_pending;

    #[test]
    fn mix_preserves_longer_monitor_chunk() {
        let mut pending = vec![10, 20, 30];
        assert_eq!(mix_with_pending(&[1], &mut pending), vec![11, 20, 30]);
        assert_eq!(pending, vec![20, 30]);
    }

    #[test]
    fn mix_clamps_samples() {
        let mut pending = vec![i16::MAX];
        assert_eq!(mix_with_pending(&[1], &mut pending), vec![i16::MAX]);
    }
}

fn get_monitor_source() -> Option<String> {
    let out = Command::new("pactl")
        .args(["list", "sources", "short"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if line.contains("monitor") && line.contains("RUNNING") {
            if let Some(parts) = line.split_whitespace().next() {
                return Some(parts.to_string());
            }
        }
    }
    for line in text.lines() {
        if line.contains("monitor") {
            if let Some(parts) = line.split_whitespace().next() {
                return Some(parts.to_string());
            }
        }
    }
    None
}

fn spawn_parec_monitor(tx: Sender<Vec<i16>>, running: Arc<AtomicBool>) -> Option<Child> {
    let monitor = get_monitor_source()?;
    let mut child = Command::new("parec")
        .arg(format!("--device={monitor}"))
        .args(["--format=s16le", "--channels=1", "--rate=16000"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    std::thread::spawn(move || {
        let mut buf = vec![0u8; audio::BLOCK * 2];
        while running.load(Ordering::Relaxed) {
            match stdout.read_exact(&mut buf) {
                Ok(_) => {
                    let samples: Vec<i16> = buf
                        .chunks_exact(2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]]))
                        .collect();
                    if tx.send(samples).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    Some(child)
}
