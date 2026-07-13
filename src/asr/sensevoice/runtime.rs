use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use half::f16;
use ort::session::Session;
use ort::value::Tensor;
use sentencepiece_rs::SentencePieceProcessor;

use crate::asr::partial::StablePrefix;
use crate::asr::{
    AsrEngine, AsrError, AsrSession, EngineCapabilities, FinalResult, Hotword, HotwordCapability,
    ModelInfo, ModelKind, OrtSessionConfig, PartialResult, RecognitionMetrics, SessionOptions,
};

use super::SenseVoiceFeatureExtractor;

pub struct SenseVoiceEngine {
    info: ModelInfo,
    runtime: Arc<Mutex<SenseVoiceRuntime>>,
}

impl SenseVoiceEngine {
    pub fn load(directory: &Path) -> Result<Self> {
        let runtime = SenseVoiceRuntime::load(directory)?;
        Ok(Self {
            info: ModelInfo {
                kind: ModelKind::SenseVoice,
                display_name: "SenseVoice-Small".into(),
            },
            runtime: Arc::new(Mutex::new(runtime)),
        })
    }
}

impl AsrEngine for SenseVoiceEngine {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    fn capabilities(&self) -> EngineCapabilities {
        EngineCapabilities {
            punctuation: true,
            timestamps: true,
            native_streaming: false,
            model_hotwords: HotwordCapability::CtcBias,
            languages: vec!["auto", "zh", "en", "ja", "ko", "yue"],
            execution_providers: vec!["cpu"],
        }
    }

    fn start_session(&mut self, options: SessionOptions) -> Result<Box<dyn AsrSession>, AsrError> {
        Ok(Box::new(SenseVoiceSession {
            session_id: options.session_id,
            runtime: Arc::clone(&self.runtime),
            hotwords: options.hotwords,
            audio: Vec::new(),
            last_partial_samples: 0,
            revision: 0,
            stable_prefix: StablePrefix::new(3),
            enable_partials: options.enable_partials,
            started_at: Instant::now(),
        }))
    }
}

struct SenseVoiceSession {
    session_id: u64,
    runtime: Arc<Mutex<SenseVoiceRuntime>>,
    hotwords: Vec<Hotword>,
    audio: Vec<f32>,
    last_partial_samples: usize,
    revision: u64,
    stable_prefix: StablePrefix,
    enable_partials: bool,
    started_at: Instant,
}

impl AsrSession for SenseVoiceSession {
    fn accept_audio(&mut self, samples: &[f32]) -> Result<(), AsrError> {
        self.audio.extend_from_slice(samples);
        Ok(())
    }

    fn poll_partial(&mut self) -> Result<Option<PartialResult>, AsrError> {
        if !self.enable_partials {
            return Ok(None);
        }
        if self.audio.len().saturating_sub(self.last_partial_samples) < 12_800 {
            return Ok(None);
        }
        self.last_partial_samples = self.audio.len();
        self.revision = self.revision.saturating_add(1);
        let candidate = self
            .runtime
            .lock()
            .map_err(|_| AsrError::Inference("SenseVoice runtime lock poisoned".into()))?
            .recognize(&self.audio, &self.hotwords)
            .map_err(|error| AsrError::Inference(error.to_string()))?;
        let text = self.stable_prefix.push(candidate);
        Ok(Some(PartialResult {
            text,
            revision: self.revision,
        }))
    }

    fn finish(self: Box<Self>) -> Result<FinalResult, AsrError> {
        let inference_started = Instant::now();
        let text = self
            .runtime
            .lock()
            .map_err(|_| AsrError::Inference("SenseVoice runtime lock poisoned".into()))?
            .recognize(&self.audio, &self.hotwords)
            .map_err(|error| AsrError::Inference(error.to_string()))?;
        Ok(FinalResult {
            text,
            metrics: RecognitionMetrics {
                audio_duration_ms: (self.audio.len() as u64 * 1000) / 16_000,
                inference_duration_ms: inference_started.elapsed().as_millis() as u64,
            },
        })
    }

    fn cancel(self: Box<Self>) {
        let _ = self.session_id;
        let _ = self.started_at;
    }
}

struct SenseVoiceRuntime {
    encoder: Session,
    decoder: Session,
    tokenizer: SentencePieceProcessor,
    feature: SenseVoiceFeatureExtractor,
}

impl SenseVoiceRuntime {
    fn load(directory: &Path) -> Result<Self> {
        let encoder_path = directory.join("SenseVoice-Encoder.fp16.onnx");
        let decoder_path = directory.join("SenseVoice-CTC.fp16.onnx");
        let tokenizer_path = directory.join("tokenizer.bpe.model");
        let ort_config = OrtSessionConfig::default();
        let encoder = crate::asr::ort_session::create_session(&encoder_path, &ort_config)
            .with_context(|| format!("failed to load {}", encoder_path.display()))?;
        let decoder = crate::asr::ort_session::create_session(&decoder_path, &ort_config)
            .with_context(|| format!("failed to load {}", decoder_path.display()))?;
        let tokenizer = SentencePieceProcessor::open(&tokenizer_path)
            .with_context(|| format!("failed to load {}", tokenizer_path.display()))?;
        Ok(Self {
            encoder,
            decoder,
            tokenizer,
            feature: SenseVoiceFeatureExtractor::new(),
        })
    }

    fn recognize(&mut self, audio: &[f32], hotwords: &[Hotword]) -> Result<String> {
        if audio.len() < 1_600 {
            return Ok(String::new());
        }
        let features = self.feature.extract(audio);
        let frame_count = features.len();
        if frame_count == 0 {
            return Ok(String::new());
        }
        let feature_values = features
            .into_iter()
            .flatten()
            .map(f16::from_f32)
            .collect::<Vec<_>>();
        let mask = vec![f16::from_f32(1.0); frame_count];
        let prompt_ids = vec![0_i64, 1, 2, 14];
        let speech = Tensor::from_array(([1, frame_count, 560], feature_values))?;
        let mask = Tensor::from_array(([1, frame_count], mask))?;
        let prompt = Tensor::from_array(([1, 4], prompt_ids))?;
        let encoder_outputs = self.encoder.run(ort::inputs! {
            "speech_feat" => speech,
            "mask" => mask,
            "prompt_ids" => prompt,
        })?;
        let enc_out = &encoder_outputs[0];
        let (shape, values) = enc_out.try_extract_tensor::<f16>()?;
        let encoder_shape = shape
            .iter()
            .map(|value| *value as usize)
            .collect::<Vec<_>>();
        let encoder_values = values.to_vec();
        drop(encoder_outputs);
        let decoder_input = Tensor::from_array((encoder_shape, encoder_values))?;
        let decoder_outputs = self.decoder.run(ort::inputs! {
            "enc_out" => decoder_input,
        })?;
        let (index_shape, indices) = decoder_outputs[1].try_extract_tensor::<i32>()?;
        let dimensions = index_shape
            .iter()
            .map(|value| *value as usize)
            .collect::<Vec<_>>();
        if dimensions.len() != 3 || dimensions[0] != 1 {
            anyhow::bail!("unexpected CTC index shape: {dimensions:?}");
        }
        let indices = indices.to_vec();
        drop(decoder_outputs);
        let time_steps = dimensions[1].min(frame_count + 4);
        let top_k = dimensions[2];
        let start = 4;
        let frame_count = time_steps.saturating_sub(start);
        let frame_indices = (0..frame_count)
            .map(|frame| {
                let offset = (frame + start) * top_k;
                &indices[offset..offset + top_k]
            })
            .collect::<Vec<_>>();
        let hits = self.detect_hotwords(&frame_indices, hotwords)?;

        let mut text = String::new();
        let mut segment_ids = Vec::new();
        let mut previous = None;
        let mut frame = 0;
        let mut hit_index = 0;
        while frame < frame_count {
            if let Some(hit) = hits.get(hit_index)
                && frame == hit.start_frame
            {
                if !segment_ids.is_empty() {
                    text.push_str(&self.tokenizer.decode_ids(&segment_ids)?);
                    segment_ids.clear();
                }
                text.push_str(&hit.text);
                frame = hit.end_frame + 1;
                previous = None;
                hit_index += 1;
                continue;
            }
            let token = frame_indices[frame][0] as usize;
            if token != 0 && previous != Some(token) {
                segment_ids.push(token);
            }
            previous = Some(token);
            frame += 1;
        }
        if !segment_ids.is_empty() {
            text.push_str(&self.tokenizer.decode_ids(&segment_ids)?);
        }
        Ok(text)
    }

    fn detect_hotwords(&self, frames: &[&[i32]], hotwords: &[Hotword]) -> Result<Vec<HotwordHit>> {
        let mut hits = Vec::new();
        for hotword in hotwords {
            let token_ids = self
                .tokenizer
                .encode_to_ids(&hotword.text)
                .with_context(|| format!("failed to tokenize hotword {}", hotword.text))?;
            if token_ids.is_empty() {
                continue;
            }
            for start in 0..frames.len() {
                let search_depth =
                    ((5.0 * hotword.boost.max(0.1)).round() as usize).clamp(1, frames[start].len());
                if !frames[start][..search_depth]
                    .iter()
                    .any(|id| *id as usize == token_ids[0])
                {
                    continue;
                }

                let mut current_frame = start;
                let mut matched = true;
                for token in token_ids.iter().skip(1) {
                    let search_end = (current_frame + 16).min(frames.len());
                    let next = ((current_frame + 1)..search_end).find(|frame| {
                        frames[*frame][..search_depth]
                            .iter()
                            .any(|id| *id as usize == *token)
                    });
                    let Some(next) = next else {
                        matched = false;
                        break;
                    };
                    current_frame = next;
                }
                if matched {
                    hits.push(HotwordHit {
                        text: hotword.text.clone(),
                        start_frame: start,
                        end_frame: current_frame,
                    });
                    break;
                }
            }
        }
        hits.sort_by_key(|hit| (hit.start_frame, usize::MAX - hit.text.len()));
        let mut selected = Vec::<HotwordHit>::new();
        for hit in hits {
            if selected
                .last()
                .map(|previous| hit.start_frame <= previous.end_frame)
                .unwrap_or(false)
            {
                continue;
            }
            selected.push(hit);
        }
        Ok(selected)
    }
}

#[derive(Debug)]
struct HotwordHit {
    text: String,
    start_frame: usize,
    end_frame: usize,
}
