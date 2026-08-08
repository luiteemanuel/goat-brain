import atexit
import io
import os
import queue
import signal
import subprocess
import threading
import time
import wave

import flask
import numpy as np
import requests
import sounddevice as sd
import webrtcvad
from flask import Flask, jsonify, request, Response, send_from_directory

BASE = os.path.dirname(os.path.abspath(__file__))
MODEL = os.path.join(BASE, "models", "ggml-large-v3-turbo.bin")
WHISPER_BIN = os.path.join(BASE, "bin", "whisper-server")
WHISPER_PORT = 8081
WHISPER_URL = f"http://127.0.0.1:{WHISPER_PORT}/inference"
APP_PORT = 5050

SR = 16000
CHANNELS = 1
FRAME_MS = 30
BLOCK = SR // 1000 * FRAME_MS
HANGOVER_FRAMES = 40
MAX_SEGMENT_SEC = 30
MIN_SEGMENT_SEC = 0.4

app = Flask(__name__)


def get_monitor_source():
    """Find the active (RUNNING) PulseAudio monitor source."""
    try:
        out = subprocess.check_output(
            ["pactl", "list", "sources", "short"], text=True, timeout=3
        )
        for line in out.strip().splitlines():
            if "monitor" in line and "RUNNING" in line:
                parts = line.split()
                if len(parts) >= 2:
                    return parts[1]
    except Exception:
        pass
    # Fallback: any monitor
    try:
        out = subprocess.check_output(
            ["pactl", "list", "sources", "short"], text=True, timeout=3
        )
        for line in out.strip().splitlines():
            if "monitor" in line:
                parts = line.split()
                if len(parts) >= 2:
                    return parts[1]
    except Exception:
        pass
    return None


class WhisperBackend:
    def __init__(self):
        self.proc = None
        self.owns = False

    def start(self):
        try:
            requests.get(WHISPER_URL, timeout=1)
            return
        except Exception:
            pass
        self.proc = subprocess.Popen(
            [WHISPER_BIN, "-m", MODEL, "-l", "pt", "-nt", "--port", str(WHISPER_PORT)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        self.owns = True
        deadline = time.time() + 30
        while time.time() < deadline:
            try:
                requests.get(WHISPER_URL, timeout=1)
                return
            except Exception:
                time.sleep(0.5)
        raise RuntimeError("whisper-server nao subiu")

    def transcribe(self, pcm_bytes: bytes, language: str) -> str:
        wav = pcm_to_wav(pcm_bytes)
        resp = requests.post(
            WHISPER_URL,
            files={"file": ("seg.wav", wav, "audio/wav")},
            data={"response_format": "json", "language": language},
            timeout=120,
        )
        resp.raise_for_status()
        return resp.json().get("text", "").strip()

    def stop(self):
        if self.proc and self.owns:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()


def pcm_to_wav(pcm_bytes: bytes) -> bytes:
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(CHANNELS)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(pcm_bytes)
    return buf.getvalue()


class MeetingRecorder:
    def __init__(self, backend):
        self.backend = backend
        self.vad = webrtcvad.Vad(2)
        self.audio_q = queue.Queue()
        self.monitor_q = queue.Queue()
        self.seg_q = queue.Queue()
        self.clients = []
        self.clients_lock = threading.Lock()
        self.stream = None
        self.monitor_proc = None
        self.monitor_thread = None
        self.running = False
        self.thread = None
        self.worker = None
        self.seg_id = 0

    def emit(self, data):
        with self.clients_lock:
            for c in self.clients:
                try:
                    c.put_nowait(data)
                except queue.Full:
                    pass

    def _callback(self, indata, frames, time_info, status):
        self.audio_q.put(indata.copy())

    def _monitor_capture(self):
        monitor = get_monitor_source()
        if not monitor:
            return
        try:
            self.monitor_proc = subprocess.Popen(
                ["parec", "--device=" + monitor,
                 "--format=s16le", "--channels=1", "--rate=" + str(SR)],
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            )
            while self.running and self.monitor_proc.poll() is None:
                chunk = self.monitor_proc.stdout.read(BLOCK * 2)
                if chunk:
                    self.monitor_q.put(chunk)
                else:
                    break
        except Exception:
            pass
        finally:
            if self.monitor_proc:
                self.monitor_proc.terminate()

    def start(self):
        if self.running:
            return
        self.running = True
        self.stream = sd.InputStream(
            samplerate=SR, channels=CHANNELS, dtype="int16",
            blocksize=BLOCK, callback=self._callback,
        )
        self.stream.start()
        self.monitor_thread = threading.Thread(target=self._monitor_capture, daemon=True)
        self.monitor_thread.start()
        self.thread = threading.Thread(target=self._vad_loop, daemon=True)
        self.thread.start()
        self.worker = threading.Thread(target=self._transcribe_loop, daemon=True)
        self.worker.start()
        self.emit({"type": "meeting_state", "state": "on"})

    def stop(self):
        if not self.running:
            return
        self.running = False
        if self.stream:
            self.stream.stop()
            self.stream.close()
            self.stream = None
        if self.monitor_proc:
            self.monitor_proc.terminate()
            try:
                self.monitor_proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                self.monitor_proc.kill()
            self.monitor_proc = None
        self.emit({"type": "meeting_state", "state": "off"})

    def _mix_chunk(self, mic_chunk, monitor_chunk):
        if not monitor_chunk:
            return mic_chunk
        # Mix mic + monitor with clipping
        n = min(len(mic_chunk), len(monitor_chunk))
        if n == 0:
            return mic_chunk
        a = np.frombuffer(mic_chunk[:n], dtype=np.int16).astype(np.int32)
        b = np.frombuffer(monitor_chunk[:n], dtype=np.int16).astype(np.int32)
        mixed = np.clip(a + b, -32768, 32767).astype(np.int16)
        return mixed.tobytes()

    def _vad_loop(self):
        buffer = bytearray()
        start_ts = time.time()
        speaking = False
        hangover = 0
        while self.running or not self.audio_q.empty():
            try:
                mic_frame = self.audio_q.get(timeout=0.3)
            except queue.Empty:
                if speaking and buffer:
                    self._flush(buffer, start_ts)
                buffer = bytearray()
                speaking = False
                hangover = 0
                continue
            mic_chunk = np.asarray(mic_frame).tobytes()
            # Get monitor chunk if available
            try:
                monitor_chunk = self.monitor_q.get_nowait()
            except queue.Empty:
                monitor_chunk = None
            chunk = self._mix_chunk(mic_chunk, monitor_chunk)
            is_speech = False
            for i in range(0, len(chunk), BLOCK * 2):
                fb = chunk[i:i + BLOCK * 2]
                if len(fb) < BLOCK * 2:
                    break
                try:
                    if self.vad.is_speech(fb, SR):
                        is_speech = True
                except Exception:
                    pass
            if is_speech:
                if not speaking:
                    speaking = True
                    start_ts = time.time()
                    buffer = bytearray()
                buffer += chunk
                hangover = 0
            else:
                if speaking:
                    buffer += chunk
                    hangover += 1
                    if hangover >= HANGOVER_FRAMES:
                        self._flush(buffer, start_ts)
                        buffer = bytearray()
                        speaking = False
                        hangover = 0
            if speaking and len(buffer) >= MAX_SEGMENT_SEC * SR * 2:
                self._flush(buffer, start_ts)
                buffer = bytearray()
                start_ts = time.time()
                hangover = 0
        if speaking and buffer:
            self._flush(buffer, start_ts)

    def _flush(self, buffer, start_ts):
        if len(buffer) < MIN_SEGMENT_SEC * SR * 2:
            return
        self.seg_id += 1
        self.emit({"type": "segment_queued", "id": self.seg_id})
        self.seg_q.put((self.seg_id, bytes(buffer), start_ts))

    def _transcribe_loop(self):
        while True:
            seg_id, pcm, ts = self.seg_q.get()
            try:
                text = self.backend.transcribe(pcm, "pt")
            except Exception as e:
                text = ""
                self.emit({"type": "error", "message": f"falha na transcricao: {e}"})
            if text:
                self.emit({
                    "type": "segment",
                    "id": seg_id,
                    "text": text,
                    "ts": time.strftime("%H:%M:%S", time.localtime(ts)),
                })


backend = WhisperBackend()
meeting = MeetingRecorder(backend)


class PromptRecorder:
    def __init__(self, backend):
        self.backend = backend
        self.buffer = bytearray()
        self.stream = None
        self.lock = threading.Lock()
        self._running = False

    def _callback(self, indata, frames, time_info, status):
        if self._running:
            self.buffer += np.asarray(indata).tobytes()

    def start(self):
        self.buffer = bytearray()
        self._running = True
        self.stream = sd.InputStream(
            samplerate=SR, channels=CHANNELS, dtype="int16",
            blocksize=BLOCK, callback=self._callback,
        )
        self.stream.start()

    def stop(self, language: str) -> str:
        self._running = False
        if self.stream:
            self.stream.stop()
            self.stream.close()
            self.stream = None
        pcm = bytes(self.buffer)
        self.buffer = bytearray()
        if len(pcm) < MIN_SEGMENT_SEC * SR * 2:
            return ""
        return self.backend.transcribe(pcm, language)


prompt = PromptRecorder(backend)


@app.get("/")
def index():
    return send_from_directory(os.path.join(BASE, "static"), "index.html")


@app.get("/api/status")
def status():
    try:
        requests.get(WHISPER_URL, timeout=1)
        whisper_ok = True
    except Exception:
        whisper_ok = False
    return jsonify({
        "whisper": whisper_ok,
        "meeting": meeting.running,
        "model": os.path.basename(MODEL),
    })


@app.post("/api/meeting/start")
def meeting_start():
    if not meeting.running:
        threading.Thread(target=meeting.start, daemon=True).start()
    return jsonify({"ok": True})


@app.post("/api/meeting/stop")
def meeting_stop():
    meeting.stop()
    return jsonify({"ok": True})


@app.post("/api/tt/start")
def tt_start():
    prompt.start()
    return jsonify({"ok": True})


@app.post("/api/tt/stop")
def tt_stop():
    language = request.json.get("language", "pt") if request.json else "pt"
    try:
        text = prompt.stop(language)
        return jsonify({"ok": True, "text": text})
    except Exception as e:
        return jsonify({"ok": False, "text": "", "error": str(e)})


@app.get("/api/events")
def events():
    q = queue.Queue(maxsize=200)
    with meeting.clients_lock:
        meeting.clients.append(q)

    def gen():
        try:
            while True:
                try:
                    data = q.get(timeout=15)
                    yield f"data: {flask.json.dumps(data)}\n\n"
                except queue.Empty:
                    yield ": keepalive\n\n"
        finally:
            with meeting.clients_lock:
                if q in meeting.clients:
                    meeting.clients.remove(q)

    return Response(gen(), mimetype="text/event-stream")


def main():
    backend.start()
    atexit.register(backend.stop)
    for sig in (signal.SIGTERM, signal.SIGINT):
        try:
            signal.signal(sig, lambda *_: (backend.stop(), os._exit(0)))
        except ValueError:
            pass
    print(f"Goat Reuniao em http://127.0.0.1:{APP_PORT}")
    app.run(host="127.0.0.1", port=APP_PORT, threaded=True, debug=False)


def open_browser():
    try:
        import webbrowser
        webbrowser.open(f"http://127.0.0.1:{APP_PORT}")
    except Exception:
        pass


if __name__ == "__main__":
    main()
