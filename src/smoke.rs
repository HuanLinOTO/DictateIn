use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use hound::{SampleFormat, WavReader};

use crate::asr::{AsrEngine, FunAsrNanoEngine, ModelKind, ModelRegistry, Qwen3AsrEngine};
use crate::asr::{SenseVoiceEngine, SessionOptions};
use crate::audio::AudioCapture;
use crate::audio::AudioResampler;

pub fn run(model_name: &str, wav_path: &Path) -> Result<()> {
    let kind = parse_model(model_name)?;
    let audio = read_wav_mono_16khz(wav_path)?;
    let registry = ModelRegistry::discover()?;
    let model_directory = registry.validate(kind)?;

    let load_started = Instant::now();
    let mut engine: Box<dyn AsrEngine> = match kind {
        ModelKind::SenseVoice => Box::new(SenseVoiceEngine::load(&model_directory)?),
        ModelKind::FunAsrNano => Box::new(FunAsrNanoEngine::load(&model_directory)?),
        ModelKind::Qwen3Asr => Box::new(Qwen3AsrEngine::load(&model_directory)?),
    };
    let load_ms = load_started.elapsed().as_millis();
    let model = engine.info().display_name.clone();
    let capabilities = engine.capabilities();
    let manifest = ModelRegistry::manifest(kind);
    let mut session = engine
        .start_session(SessionOptions {
            session_id: 1,
            hotwords: Vec::new(),
            enable_partials: false,
        })
        .map_err(anyhow::Error::msg)?;
    session.accept_audio(&audio).map_err(anyhow::Error::msg)?;
    let result = session.finish().map_err(anyhow::Error::msg)?;

    println!("model: {model}");
    println!("model_kind: {:?}", manifest.kind);
    println!("model_version: {}", manifest.version);
    println!("model_license: {}", manifest.license);
    println!("sample_rate: {}", manifest.sample_rate);
    println!("hotwords: {:?}", capabilities.model_hotwords);
    println!("manifest_capabilities: {:?}", manifest.capabilities);
    println!("samples: {}", audio.len());
    println!("audio_ms: {}", result.metrics.audio_duration_ms);
    println!("load_ms: {load_ms}");
    println!("inference_ms: {}", result.metrics.inference_duration_ms);
    println!("text: {}", result.text);
    Ok(())
}

pub fn run_audio() -> Result<()> {
    let (sender, receiver) = crossbeam_channel::bounded(128);
    let capture = AudioCapture::open(None, sender)?;
    println!(
        "microphone: {} @ {} Hz",
        capture.device_name, capture.sample_rate
    );
    capture.start()?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    capture.pause()?;
    println!("captured_blocks: {}", receiver.len());
    Ok(())
}

fn parse_model(value: &str) -> Result<ModelKind> {
    match value.to_ascii_lowercase().as_str() {
        "sensevoice" | "sensevoice-small" => Ok(ModelKind::SenseVoice),
        "fun-asr-nano" | "funasr" => Ok(ModelKind::FunAsrNano),
        "qwen3-asr" | "qwen" => Ok(ModelKind::Qwen3Asr),
        _ => bail!("unknown model '{value}'"),
    }
}

fn read_wav_mono_16khz(path: &Path) -> Result<Vec<f32>> {
    let mut reader =
        WavReader::open(path).with_context(|| format!("failed to open WAV {}", path.display()))?;
    let spec = reader.spec();
    if spec.channels == 0 || spec.sample_rate == 0 {
        bail!("WAV has an invalid channel count or sample rate");
    }

    let interleaved = match spec.sample_format {
        SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .context("failed to decode floating-point WAV samples")?,
        SampleFormat::Int => {
            let scale = 2_f32.powi(i32::from(spec.bits_per_sample.saturating_sub(1)));
            reader
                .samples::<i32>()
                .map(|sample| sample.map(|value| value as f32 / scale))
                .collect::<Result<Vec<_>, _>>()
                .context("failed to decode integer WAV samples")?
        }
    };
    let channels = usize::from(spec.channels);
    let mono = interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect::<Vec<_>>();
    let mut resampler = AudioResampler::new(spec.sample_rate, 16_000)?;
    let mut output = resampler.process(&mono)?;
    output.extend(resampler.finish()?);
    Ok(output)
}
