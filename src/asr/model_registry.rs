use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::asr::{EngineCapabilities, HotwordCapability, ModelKind};
use crate::paths::AppPaths;

#[derive(Debug, Clone)]
pub struct ModelFile {
    pub name: &'static str,
    pub minimum_size: u64,
}

#[derive(Debug, Clone)]
pub struct ModelManifest {
    pub id: &'static str,
    pub kind: ModelKind,
    pub version: &'static str,
    pub files: Vec<ModelFile>,
    pub license: &'static str,
    pub sample_rate: u32,
    pub capabilities: EngineCapabilities,
}

impl ModelManifest {
    pub fn validate(&self, directory: &Path) -> Result<()> {
        if !directory.is_dir() {
            bail!("model directory does not exist: {}", directory.display());
        }

        for required in &self.files {
            let path = directory.join(required.name);
            let metadata = fs::metadata(&path)
                .with_context(|| format!("missing model file: {}", path.display()))?;
            if metadata.len() < required.minimum_size {
                bail!(
                    "model file is too small: {} ({} bytes)",
                    path.display(),
                    metadata.len()
                );
            }
        }
        Ok(())
    }
}

pub struct ModelRegistry {
    root: PathBuf,
}

impl ModelRegistry {
    pub fn discover() -> Result<Self> {
        let paths = AppPaths::discover()?;
        paths.ensure_directories()?;
        Ok(Self { root: paths.models })
    }

    #[cfg(test)]
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn manifest(kind: ModelKind) -> ModelManifest {
        match kind {
            ModelKind::SenseVoice => sensevoice_manifest(),
            ModelKind::FunAsrNano => fun_asr_manifest(),
            ModelKind::Qwen3Asr => qwen_manifest(),
        }
    }

    pub fn model_directory(&self, kind: ModelKind) -> PathBuf {
        self.root.join(Self::manifest(kind).id)
    }

    pub fn validate(&self, kind: ModelKind) -> Result<PathBuf> {
        let directory = self.model_directory(kind);
        Self::manifest(kind).validate(&directory)?;
        Ok(directory)
    }
}

fn sensevoice_manifest() -> ModelManifest {
    ModelManifest {
        id: "sensevoice-small",
        kind: ModelKind::SenseVoice,
        version: "capswriter-2.5-reference",
        files: vec![
            ModelFile {
                name: "SenseVoice-Encoder.fp16.onnx",
                minimum_size: 1024,
            },
            ModelFile {
                name: "SenseVoice-CTC.fp16.onnx",
                minimum_size: 1024,
            },
            ModelFile {
                name: "tokenizer.bpe.model",
                minimum_size: 1024,
            },
        ],
        license: "model-specific; verify before redistribution",
        sample_rate: 16_000,
        capabilities: EngineCapabilities {
            punctuation: true,
            timestamps: true,
            native_streaming: false,
            model_hotwords: HotwordCapability::CtcBias,
            languages: vec!["auto", "zh", "en", "ja", "ko", "yue"],
            execution_providers: vec!["cpu", "directml"],
        },
    }
}

fn fun_asr_manifest() -> ModelManifest {
    ModelManifest {
        id: "fun-asr-nano",
        kind: ModelKind::FunAsrNano,
        version: "capswriter-2.5-reference",
        files: vec![
            ModelFile {
                name: "Fun-ASR-Nano-Encoder-Adaptor.fp16.onnx",
                minimum_size: 1024,
            },
            ModelFile {
                name: "Fun-ASR-Nano-CTC.fp16.onnx",
                minimum_size: 1024,
            },
            ModelFile {
                name: "Fun-ASR-Nano-Decoder.q5_k.gguf",
                minimum_size: 1024,
            },
            ModelFile {
                name: "tokens.txt",
                minimum_size: 1,
            },
        ],
        license: "model-specific; verify before redistribution",
        sample_rate: 16_000,
        capabilities: EngineCapabilities {
            punctuation: true,
            timestamps: true,
            native_streaming: false,
            model_hotwords: HotwordCapability::PromptContext,
            languages: vec!["zh", "en"],
            execution_providers: vec!["cpu", "directml", "gpu_offload"],
        },
    }
}

fn qwen_manifest() -> ModelManifest {
    ModelManifest {
        id: "qwen3-asr",
        kind: ModelKind::Qwen3Asr,
        version: "capswriter-2.5-reference",
        files: vec![
            ModelFile {
                name: "qwen3_asr_encoder_frontend.int4.onnx",
                minimum_size: 1024,
            },
            ModelFile {
                name: "qwen3_asr_encoder_backend.int4.onnx",
                minimum_size: 1024,
            },
            ModelFile {
                name: "qwen3_asr_llm.q4_k.gguf",
                minimum_size: 1024,
            },
        ],
        license: "model-specific; verify before redistribution",
        sample_rate: 16_000,
        capabilities: EngineCapabilities {
            punctuation: true,
            timestamps: false,
            native_streaming: false,
            model_hotwords: HotwordCapability::PromptContext,
            languages: vec!["auto"],
            execution_providers: vec!["cpu", "directml", "gpu_offload"],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_missing_model_file() {
        let root =
            std::env::temp_dir().join(format!("dictate-in-model-test-{}", std::process::id()));
        let directory = root.join("sensevoice-small");
        fs::create_dir_all(&directory).unwrap();
        let registry = ModelRegistry::with_root(root.clone());

        let error = registry.validate(ModelKind::SenseVoice).unwrap_err();
        assert!(error.to_string().contains("missing model file"));
        let _ = fs::remove_dir_all(root);
    }
}
