use anyhow::{anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, SizedSample, Stream, StreamConfig};

pub const SR: u32 = 16000;
pub const FRAME_MS: usize = 30;
pub const BLOCK: usize = SR as usize * FRAME_MS / 1000;

trait AsI16: SizedSample {
    fn to_i16(self) -> i16;
}

impl AsI16 for i16 {
    fn to_i16(self) -> i16 {
        self
    }
}
impl AsI16 for i32 {
    fn to_i16(self) -> i16 {
        (self >> 16) as i16
    }
}
impl AsI16 for f32 {
    fn to_i16(self) -> i16 {
        (self * 32767.0) as i16
    }
}
impl AsI16 for f64 {
    fn to_i16(self) -> i16 {
        (self * 32767.0) as i16
    }
}
impl AsI16 for u8 {
    fn to_i16(self) -> i16 {
        ((self as i32 - 128) << 8) as i16
    }
}
impl AsI16 for u16 {
    fn to_i16(self) -> i16 {
        self as i16
    }
}

pub fn open_mic_stream<F>(cb: F) -> anyhow::Result<Stream>
where
    F: FnMut(Vec<i16>) + Send + 'static,
{
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("nenhum dispositivo de entrada encontrado"))?;
    open_device_stream(&device, cb)
}

pub fn find_monitor_device() -> Option<Device> {
    let host = cpal::default_host();
    let devs = host.input_devices().ok()?;
    for d in devs {
        if let Ok(desc) = d.description() {
            if desc.name().to_lowercase().contains("monitor") {
                return Some(d);
            }
        }
    }
    None
}

pub fn open_device_stream<F>(device: &Device, cb: F) -> anyhow::Result<Stream>
where
    F: FnMut(Vec<i16>) + Send + 'static,
{
    let cfg = pick_config(device)?;
    if let Ok(desc) = device.description() {
        eprintln!(
            "[goat] abrindo entrada: {} @ {}Hz {} (default? {})",
            desc.name(),
            cfg.sample_rate(),
            cfg.sample_format(),
            is_default_input(device)
        );
    }
    let stream_config: StreamConfig = cfg.into();
    let mut stream_config = stream_config;
    stream_config.channels = 1;
    match cfg.sample_format() {
        SampleFormat::I16 => build::<i16, _>(device, stream_config, cb),
        SampleFormat::I32 => build::<i32, _>(device, stream_config, cb),
        SampleFormat::F32 => build::<f32, _>(device, stream_config, cb),
        SampleFormat::F64 => build::<f64, _>(device, stream_config, cb),
        SampleFormat::U8 => build::<u8, _>(device, stream_config, cb),
        SampleFormat::U16 => build::<u16, _>(device, stream_config, cb),
        other => bail!("formato de áudio não suportado: {other}"),
    }
}

fn is_default_input(device: &Device) -> bool {
    let host = cpal::default_host();
    match host.default_input_device() {
        Some(d) => match (d.id(), device.id()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        },
        None => false,
    }
}

pub fn list_input_devices() {
    let host = cpal::default_host();
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            if let Ok(desc) = d.description() {
                eprintln!("[goat] entrada disponível: {}", desc.name());
            }
        }
    }
}

fn pick_config(device: &Device) -> anyhow::Result<cpal::SupportedStreamConfig> {
    let mut preferred: Option<cpal::SupportedStreamConfig> = None;
    for range in device.supported_input_configs()? {
        if range.channels() == 1 && range.min_sample_rate() <= SR && range.max_sample_rate() >= SR {
            let cfg = range.with_sample_rate(SR);
            if cfg.sample_format() == SampleFormat::I16 {
                return Ok(cfg);
            }
            if preferred.is_none() {
                preferred = Some(cfg);
            }
        }
    }
    if let Some(cfg) = preferred {
        return Ok(cfg);
    }
    device
        .default_input_config()
        .map_err(|e| anyhow!("config padrão indisponível: {e}"))
}

fn build<T, F>(device: &Device, config: StreamConfig, mut cb: F) -> anyhow::Result<Stream>
where
    T: AsI16 + Copy,
    F: FnMut(Vec<i16>) + Send + 'static,
{
    let channels = config.channels.max(1) as usize;
    let rate = config.sample_rate as f64;
    let ratio = rate / SR as f64;
    let stream = device
        .build_input_stream::<T, _, _>(
            config,
            move |data, _| {
                let mut acc = 0.0f64;
                let mut out: Vec<i16> = Vec::with_capacity(BLOCK);
                for frame in data.chunks_exact(channels) {
                    acc += 1.0;
                    if acc < ratio {
                        continue;
                    }
                    acc -= ratio;
                    let mut sum: i32 = 0;
                    for &s in frame {
                        sum += s.to_i16() as i32;
                    }
                    out.push((sum / channels as i32) as i16);
                }
                if !out.is_empty() {
                    cb(out);
                }
            },
            |e| eprintln!("[goat] erro de áudio: {e}"),
            None,
        )
        .map_err(|e| anyhow!("falha ao abrir stream: {e}"))?;
    stream
        .play()
        .map_err(|e| anyhow!("falha ao iniciar stream: {e}"))?;
    Ok(stream)
}
