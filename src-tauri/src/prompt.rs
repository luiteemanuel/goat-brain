use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::audio;
use crate::whisper::WhisperBackend;

const MIN_SEGMENT_SAMPLES: usize = 400;

pub struct PromptRecorder {
    backend: Arc<WhisperBackend>,
    stream: Mutex<Option<cpal::Stream>>,
    buffer: Arc<Mutex<Vec<i16>>>,
    recording: Arc<AtomicBool>,
}

impl PromptRecorder {
    pub fn new(backend: Arc<WhisperBackend>) -> Self {
        Self {
            backend,
            stream: Mutex::new(None),
            buffer: Arc::new(Mutex::new(Vec::new())),
            recording: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(&self) -> anyhow::Result<()> {
        let mut stream_slot = self.stream.lock().unwrap();
        if stream_slot.is_some() {
            return Ok(());
        }

        // Open the device before publishing the recording state. A failed device
        // open must not leave the UI believing that PTT is active.
        let buf = self.buffer.clone();
        let rec = self.recording.clone();
        let stream = audio::open_mic_stream(move |chunk| {
            if rec.load(Ordering::Acquire) {
                buf.lock().unwrap().extend_from_slice(&chunk);
            }
        })?;
        self.buffer.lock().unwrap().clear();
        self.recording.store(true, Ordering::Release);
        *stream_slot = Some(stream);
        Ok(())
    }

    pub fn stop(&self, language: &str) -> anyhow::Result<Option<String>> {
        let mut stream_slot = self.stream.lock().unwrap();
        if stream_slot.is_none() {
            return Ok(None);
        }
        // Signal the callback to stop extending the buffer, then drop the
        // stream so all in-flight callbacks finish before we consume the buffer.
        self.recording.store(false, Ordering::Release);
        stream_slot.take();
        drop(stream_slot);

        let pcm = std::mem::take(&mut *self.buffer.lock().unwrap());
        if pcm.len() < MIN_SEGMENT_SAMPLES {
            return Ok(Some(String::new()));
        }
        self.backend.transcribe(&pcm, language).map(Some)
    }

    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }
}
