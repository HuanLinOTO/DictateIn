use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use ort::session::Session;
use ort::value::Tensor;

use crate::asr::llama_runtime::{LlamaEmbeddingRuntime, gpu_layer_count};
use crate::asr::partial::StablePrefix;
use crate::asr::{
    AsrEngine, AsrError, AsrSession, EngineCapabilities, FinalResult, Hotword, HotwordCapability,
    ModelInfo, ModelKind, OrtSessionConfig, PartialResult, RecognitionMetrics, SessionOptions,
};

use super::feature::QwenMelExtractor;

pub struct Qwen3AsrEngine {
    info: ModelInfo,
    runtime: Arc<Mutex<QwenRuntime>>,
}

impl Qwen3AsrEngine {
    pub fn load(directory: &Path) -> Result<Self> {
        Ok(Self {
            info: ModelInfo {
                kind: ModelKind::Qwen3Asr,
                display_name: "Qwen3-ASR".into(),
            },
            runtime: Arc::new(Mutex::new(QwenRuntime::load(directory)?)),
        })
    }
}

impl AsrEngine for Qwen3AsrEngine {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn capabilities(&self) -> EngineCapabilities {
        EngineCapabilities {
            punctuation: true,
            timestamps: false,
            native_streaming: false,
            model_hotwords: HotwordCapability::PromptContext,
            languages: vec!["auto"],
            execution_providers: vec!["cpu", "gpu_offload"],
        }
    }

    fn start_session(&mut self, options: SessionOptions) -> Result<Box<dyn AsrSession>, AsrError> {
        Ok(Box::new(QwenSession {
            runtime: Arc::clone(&self.runtime),
            audio: Vec::new(),
            hotwords: options.hotwords,
            last_partial_samples: 0,
            revision: 0,
            stable_prefix: StablePrefix::new(2),
            enable_partials: options.enable_partials,
        }))
    }
}

struct QwenSession {
    runtime: Arc<Mutex<QwenRuntime>>,
    audio: Vec<f32>,
    hotwords: Vec<Hotword>,
    last_partial_samples: usize,
    revision: u64,
    stable_prefix: StablePrefix,
    enable_partials: bool,
}

impl AsrSession for QwenSession {
    fn accept_audio(&mut self, samples: &[f32]) -> Result<(), AsrError> {
        self.audio.extend_from_slice(samples);
        Ok(())
    }

    fn poll_partial(&mut self) -> Result<Option<PartialResult>, AsrError> {
        if !self.enable_partials {
            return Ok(None);
        }
        if self.audio.len().saturating_sub(self.last_partial_samples) < 32_000 {
            return Ok(None);
        }
        self.last_partial_samples = self.audio.len();
        self.revision += 1;
        let candidate = self
            .runtime
            .lock()
            .map_err(|_| AsrError::Inference("Qwen runtime lock poisoned".into()))?
            .recognize(&self.audio, &self.hotwords, 128)
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
            .map_err(|_| AsrError::Inference("Qwen runtime lock poisoned".into()))?
            .recognize(&self.audio, &self.hotwords, 512)
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

struct QwenRuntime {
    frontend: Session,
    backend: Session,
    llama: LlamaEmbeddingRuntime,
    mel: QwenMelExtractor,
}

impl QwenRuntime {
    fn load(directory: &Path) -> Result<Self> {
        let ort_config = OrtSessionConfig::default();
        let frontend = crate::asr::ort_session::create_session(
            &directory.join("qwen3_asr_encoder_frontend.int4.onnx"),
            &ort_config,
        )?;
        let backend = crate::asr::ort_session::create_session(
            &directory.join("qwen3_asr_encoder_backend.int4.onnx"),
            &ort_config,
        )?;
        let gpu_layers = gpu_layer_count();
        let llama = LlamaEmbeddingRuntime::load(
            &directory.join("qwen3_asr_llm.q4_k.gguf"),
            8192,
            gpu_layers,
        )?;
        Ok(Self {
            frontend,
            backend,
            llama,
            mel: QwenMelExtractor::new(),
        })
    }

    fn recognize(
        &mut self,
        audio: &[f32],
        hotwords: &[Hotword],
        maximum_tokens: usize,
    ) -> Result<String> {
        if audio.len() < 1_600 {
            return Ok(String::new());
        }
        let audio_embeddings = self.encode(audio)?;
        let audio_count = audio_embeddings.len() / self.llama.embedding_size();
        let context = if hotwords.is_empty() {
            "You are a helpful assistant.".to_string()
        } else {
            format!(
                "You are a helpful assistant. Transcription context terms: {}",
                hotwords
                    .iter()
                    .map(|hotword| hotword.text.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let prefix =
            format!("<|im_start|>system\n{context}<|im_end|><|im_start|>user\n<|audio_start|>");
        let suffix = "<|audio_end|><|im_end|><|im_start|>assistant\n<asr_text>";
        let prefix_tokens = self.llama.tokenize(&prefix, true)?;
        let suffix_tokens = self.llama.tokenize(suffix, true)?;

        self.llama.clear();
        self.llama.decode_tokens(&prefix_tokens, 0, false)?;
        let audio_start = prefix_tokens.len() as i32;
        let base_positions = (audio_start..audio_start + audio_count as i32).collect::<Vec<_>>();
        let mut positions = Vec::with_capacity(audio_count * 4);
        positions.extend_from_slice(&base_positions);
        positions.extend_from_slice(&base_positions);
        positions.extend_from_slice(&base_positions);
        positions.extend(std::iter::repeat_n(0, audio_count));
        self.llama
            .decode_embeddings(&audio_embeddings, audio_count, &positions, false)?;
        let suffix_start = audio_start + audio_count as i32;
        self.llama
            .decode_tokens(&suffix_tokens, suffix_start, true)?;
        self.llama.generate_greedy(
            suffix_start + suffix_tokens.len() as i32,
            maximum_tokens,
            &[151643, 151645],
        )
    }

    fn encode(&mut self, audio: &[f32]) -> Result<Vec<f32>> {
        let mel = self.mel.extract(audio);
        let frames = audio.len() / 160;
        let padded_frames = frames.div_ceil(100) * 100;
        let mut padded = vec![0.0_f32; 128 * padded_frames];
        for mel_index in 0..128 {
            let source = &mel[mel_index * frames..(mel_index + 1) * frames];
            let target = &mut padded[mel_index * padded_frames..mel_index * padded_frames + frames];
            target.copy_from_slice(source);
        }

        let mut hidden = Vec::<f32>::new();
        let mut hidden_dimension = 0;
        for chunk_index in 0..padded_frames / 100 {
            let mut chunk = Vec::with_capacity(128 * 100);
            for mel_index in 0..128 {
                let start = mel_index * padded_frames + chunk_index * 100;
                chunk.extend_from_slice(&padded[start..start + 100]);
            }
            let input = Tensor::from_array(([1, 128, 100], chunk))?;
            let outputs = self.frontend.run(ort::inputs! {
                "chunk_mel" => input,
            })?;
            let (shape, values) = outputs[0].try_extract_tensor::<f32>()?;
            hidden_dimension = shape[2] as usize;
            hidden.extend_from_slice(values);
        }
        let valid_hidden = qwen_output_length(frames).min(hidden.len() / hidden_dimension);
        hidden.truncate(valid_hidden * hidden_dimension);
        let hidden_tensor = Tensor::from_array(([1, valid_hidden, hidden_dimension], hidden))?;
        let attention = vec![0.0_f32; valid_hidden * valid_hidden];
        let attention_tensor = Tensor::from_array(([1, 1, valid_hidden, valid_hidden], attention))?;
        let outputs = self.backend.run(ort::inputs! {
            "hidden_states" => hidden_tensor,
            "attention_mask" => attention_tensor,
        })?;
        let (_, values) = outputs[0].try_extract_tensor::<f32>()?;
        Ok(values.to_vec())
    }
}

fn qwen_output_length(input_frames: usize) -> usize {
    let remainder = input_frames % 100;
    if remainder == 0 {
        return input_frames / 100 * 13;
    }
    let feature = remainder.saturating_sub(1) / 2 + 1;
    let tail = (feature.saturating_sub(1) / 2 + 1).saturating_sub(1) / 2 + 1;
    tail + input_frames / 100 * 13
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qwen_frontend_length_matches_atomic_chunks() {
        assert_eq!(qwen_output_length(100), 13);
        assert_eq!(qwen_output_length(200), 26);
    }
}
