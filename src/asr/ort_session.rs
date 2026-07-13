use anyhow::Result;
use ort::session::{Session, builder::GraphOptimizationLevel};
use std::path::Path;

pub struct OrtSessionConfig {
    pub use_gpu: bool,
}

impl Default for OrtSessionConfig {
    fn default() -> Self {
        Self { use_gpu: true }
    }
}

fn ort_err(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("ONNX Runtime error: {e}")
}

pub fn create_session(path: &Path, config: &OrtSessionConfig) -> Result<Session> {
    let builder = Session::builder()
        .map_err(ort_err)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| ort_err(e.to_string()))?
        .with_intra_threads(
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4),
        )
        .map_err(|e| ort_err(e.to_string()))?;

    #[allow(unused_mut)]
    let mut builder = builder;

    #[cfg(feature = "ort-cuda")]
    if config.use_gpu {
        builder = builder
            .with_execution_providers([
                ort::ep::cuda::CUDA::default().build().fail_silently(),
            ])
            .map_err(|e| ort_err(e.to_string()))?;
    }

    #[cfg(feature = "ort-directml")]
    if config.use_gpu {
        builder = builder
            .with_execution_providers([
                ort::ep::directml::DirectML::default().build().fail_silently(),
            ])
            .map_err(|e| ort_err(e.to_string()))?;
    }

    #[cfg(feature = "ort-coreml")]
    if config.use_gpu {
        builder = builder
            .with_execution_providers([
                ort::ep::coreml::CoreML::default().build().fail_silently(),
            ])
            .map_err(|e| ort_err(e.to_string()))?;
    }

    #[cfg(feature = "ort-rocm")]
    if config.use_gpu {
        builder = builder
            .with_execution_providers([
                ort::ep::rocm::ROCm::default().build().fail_silently(),
            ])
            .map_err(|e| ort_err(e.to_string()))?;
    }

    let session = builder
        .commit_from_file(path)
        .map_err(|e| ort_err(e.to_string()))?;
    Ok(session)
}
