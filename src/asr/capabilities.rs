#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotwordCapability {
    CtcBias,
    PromptContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineCapabilities {
    pub punctuation: bool,
    pub timestamps: bool,
    pub native_streaming: bool,
    pub model_hotwords: HotwordCapability,
    pub languages: Vec<&'static str>,
    pub execution_providers: Vec<&'static str>,
}
