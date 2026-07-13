use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use base64::Engine as _;
use half::f16;
use ort::session::Session;
use ort::value::Tensor;

use crate::asr::llama_runtime::{LlamaEmbeddingRuntime, gpu_layer_count};
use crate::asr::partial::StablePrefix;
use crate::asr::sensevoice::SenseVoiceFeatureExtractor;
use crate::asr::{
    AsrEngine, AsrError, AsrSession, EngineCapabilities, FinalResult, Hotword, HotwordCapability,
    ModelInfo, ModelKind, OrtSessionConfig, PartialResult, RecognitionMetrics, SessionOptions,
};

pub struct FunAsrNanoEngine {
    info: ModelInfo,
    runtime: Arc<Mutex<FunAsrRuntime>>,
}

impl FunAsrNanoEngine {
    pub fn load(directory: &Path) -> Result<Self> {
        Ok(Self {
            info: ModelInfo {
                kind: ModelKind::FunAsrNano,
                display_name: "Fun-ASR-Nano".into(),
            },
            runtime: Arc::new(Mutex::new(FunAsrRuntime::load(directory)?)),
        })
    }
}

impl AsrEngine for FunAsrNanoEngine {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn capabilities(&self) -> EngineCapabilities {
        EngineCapabilities {
            punctuation: true,
            timestamps: true,
            native_streaming: false,
            model_hotwords: HotwordCapability::PromptContext,
            languages: vec!["zh", "en"],
            execution_providers: vec!["cpu", "gpu_offload"],
        }
    }

    fn start_session(&mut self, options: SessionOptions) -> Result<Box<dyn AsrSession>, AsrError> {
        Ok(Box::new(FunAsrSession {
            runtime: Arc::clone(&self.runtime),
            audio: Vec::new(),
            hotwords: options.hotwords,
            revision: 0,
            last_partial_samples: 0,
            stable_prefix: StablePrefix::new(3),
            enable_partials: options.enable_partials,
        }))
    }
}

struct FunAsrSession {
    runtime: Arc<Mutex<FunAsrRuntime>>,
    audio: Vec<f32>,
    hotwords: Vec<Hotword>,
    revision: u64,
    last_partial_samples: usize,
    stable_prefix: StablePrefix,
    enable_partials: bool,
}

impl AsrSession for FunAsrSession {
    fn accept_audio(&mut self, samples: &[f32]) -> Result<(), AsrError> {
        self.audio.extend_from_slice(samples);
        Ok(())
    }

    fn poll_partial(&mut self) -> Result<Option<PartialResult>, AsrError> {
        if !self.enable_partials {
            return Ok(None);
        }
        if self.audio.len().saturating_sub(self.last_partial_samples) < 16_000 {
            return Ok(None);
        }
        self.last_partial_samples = self.audio.len();
        self.revision += 1;
        let candidate = self
            .runtime
            .lock()
            .map_err(|_| AsrError::Inference("Fun-ASR runtime lock poisoned".into()))?
            .ctc_partial(&self.audio)
            .map_err(|error| AsrError::Inference(error.to_string()))?;
        let text = self.stable_prefix.push(candidate);
        Ok(Some(PartialResult {
            text,
            revision: self.revision,
        }))
    }

    fn finish(self: Box<Self>) -> Result<FinalResult, AsrError> {
        let started = Instant::now();
        let text = self
            .runtime
            .lock()
            .map_err(|_| AsrError::Inference("Fun-ASR runtime lock poisoned".into()))?
            .recognize(&self.audio, &self.hotwords)
            .map_err(|error| AsrError::Inference(error.to_string()))?;
        Ok(FinalResult {
            text,
            metrics: RecognitionMetrics {
                audio_duration_ms: self.audio.len() as u64 * 1000 / 16_000,
                inference_duration_ms: started.elapsed().as_millis() as u64,
            },
        })
    }

    fn cancel(self: Box<Self>) {}
}

struct FunAsrRuntime {
    encoder: Session,
    ctc: Session,
    llama: LlamaEmbeddingRuntime,
    feature: SenseVoiceFeatureExtractor,
    tokens: HashMap<usize, String>,
    blank_id: usize,
}

impl FunAsrRuntime {
    fn load(directory: &Path) -> Result<Self> {
        let encoder_path = directory.join("Fun-ASR-Nano-Encoder-Adaptor.fp16.onnx");
        let ctc_path = directory.join("Fun-ASR-Nano-CTC.fp16.onnx");
        let gguf_path = directory.join("Fun-ASR-Nano-Decoder.q5_k.gguf");
        let tokens = load_tokens(&directory.join("tokens.txt"))?;
        let blank_id = tokens
            .iter()
            .find(|(_, token)| matches!(token.trim(), "<blk>" | "<blank>" | "<pad>"))
            .map(|(id, _)| *id)
            .or_else(|| tokens.keys().max().copied())
            .unwrap_or(0);
        let ort_config = OrtSessionConfig::default();
        let encoder = crate::asr::ort_session::create_session(&encoder_path, &ort_config)?;
        let ctc = crate::asr::ort_session::create_session(&ctc_path, &ort_config)?;
        let gpu_layers = gpu_layer_count();
        let llama = LlamaEmbeddingRuntime::load(&gguf_path, 2048, gpu_layers)?;
        Ok(Self {
            encoder,
            ctc,
            llama,
            feature: SenseVoiceFeatureExtractor::new(),
            tokens,
            blank_id,
        })
    }

    fn ctc_partial(&mut self, audio: &[f32]) -> Result<String> {
        let (_, encoder_output) = self.encode(audio)?;
        self.decode_ctc(encoder_output)
    }

    fn recognize(&mut self, audio: &[f32], hotwords: &[Hotword]) -> Result<String> {
        if audio.len() < 1_600 {
            return Ok(String::new());
        }
        let (audio_embeddings, encoder_output) = self.encode(audio)?;
        let ctc_text = self.decode_ctc(encoder_output)?;
        let hotword_text = hotwords
            .iter()
            .map(|hotword| hotword.text.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let mut prefix =
            "<|im_start|>system\nYou are a helpful assistant.<|im_end|>\n<|im_start|>user\n"
                .to_string();
        if !hotword_text.is_empty() {
            prefix.push_str(&format!("热词列表：[{hotword_text}]\n"));
        }
        prefix.push_str("语音转写：");
        let suffix = "<|im_end|>\n<|im_start|>assistant\n";
        let prefix_tokens = self.llama.tokenize(&prefix, true)?;
        let suffix_tokens = self.llama.tokenize(suffix, true)?;
        let audio_count = audio_embeddings.len() / self.llama.embedding_size();

        self.llama.clear();
        self.llama.decode_tokens(&prefix_tokens, 0, false)?;
        let audio_start = prefix_tokens.len() as i32;
        let positions = (audio_start..audio_start + audio_count as i32).collect::<Vec<_>>();
        self.llama
            .decode_embeddings(&audio_embeddings, audio_count, &positions, false)?;
        let suffix_start = audio_start + audio_count as i32;
        self.llama
            .decode_tokens(&suffix_tokens, suffix_start, true)?;
        let generated = self.llama.generate_greedy(
            suffix_start + suffix_tokens.len() as i32,
            256,
            &[151643, 151645],
        )?;
        let generated = generated.trim().to_string();
        if generated.is_empty() {
            Ok(ctc_text)
        } else {
            Ok(generated)
        }
    }

    fn encode(&mut self, audio: &[f32]) -> Result<(Vec<f32>, EncoderOutput)> {
        let features = self.feature.extract(audio);
        let frames = features.len();
        let values = features
            .into_iter()
            .flatten()
            .map(f16::from_f32)
            .collect::<Vec<_>>();
        let mask = vec![f16::from_f32(1.0); frames];
        let feature_tensor = Tensor::from_array(([1, frames, 560], values))?;
        let mask_tensor = Tensor::from_array(([1, frames], mask))?;
        let outputs = self.encoder.run(ort::inputs! {
            "lfr_feat" => feature_tensor,
            "mask" => mask_tensor,
        })?;
        let (encoder_shape, encoder_values) = outputs[0].try_extract_tensor::<f16>()?;
        let encoder_shape = encoder_shape.iter().map(|value| *value as usize).collect();
        let encoder_values = encoder_values.to_vec();
        let (adaptor_shape, adaptor_values) = outputs[1].try_extract_tensor::<f16>()?;
        let adaptor_shape = adaptor_shape
            .iter()
            .map(|value| *value as usize)
            .collect::<Vec<_>>();
        let mut adaptor = adaptor_values
            .iter()
            .map(|value| value.to_f32())
            .collect::<Vec<_>>();
        let target_length = fun_adaptor_length(audio.len());
        let embedding_size = adaptor_shape[2];
        adaptor.truncate(target_length.min(adaptor_shape[1]) * embedding_size);
        Ok((
            adaptor,
            EncoderOutput {
                shape: encoder_shape,
                values: encoder_values,
            },
        ))
    }

    fn decode_ctc(&mut self, encoder: EncoderOutput) -> Result<String> {
        let input = Tensor::from_array((encoder.shape, encoder.values))?;
        let outputs = self.ctc.run(ort::inputs! {
            "enc_output" => input,
        })?;
        let (shape, indices) = outputs[1].try_extract_tensor::<i32>()?;
        let dimensions = shape
            .iter()
            .map(|value| *value as usize)
            .collect::<Vec<_>>();
        if dimensions.len() != 3 {
            anyhow::bail!("unexpected Fun-ASR CTC shape: {dimensions:?}");
        }
        let top_k = dimensions[2];
        let mut previous = None;
        let mut text = String::new();
        for frame in 0..dimensions[1] {
            let token = indices[frame * top_k] as usize;
            if token != self.blank_id
                && previous != Some(token)
                && let Some(piece) = self.tokens.get(&token)
            {
                text.push_str(piece);
            }
            previous = Some(token);
        }
        Ok(text)
    }
}

struct EncoderOutput {
    shape: Vec<usize>,
    values: Vec<f16>,
}

fn fun_adaptor_length(audio_samples: usize) -> usize {
    let mel = audio_samples / 160 + 1;
    let lfr = mel.div_ceil(6);
    let first = 1 + lfr.saturating_sub(1) / 2;
    let second = 1 + first.saturating_sub(1) / 2;
    second.saturating_sub(1) / 2 + 1
}

fn load_tokens(path: &Path) -> Result<HashMap<usize, String>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut tokens = HashMap::new();
    for line in content.lines() {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        if parts.is_empty() {
            continue;
        }
        let (encoded, id) = if parts.len() == 1 {
            ("", parts[0])
        } else {
            (parts[0], parts[1])
        };
        let id = id.parse::<usize>()?;
        let token = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_else(|| encoded.to_string());
        tokens.insert(id, if parts.len() == 1 { " ".into() } else { token });
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptor_length_is_bounded() {
        let length = fun_adaptor_length(16_000);
        assert!((3..=6).contains(&length));
    }
}
