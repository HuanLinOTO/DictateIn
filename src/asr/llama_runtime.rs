use std::ffi::CString;
use std::path::Path;
use std::ptr;
use std::sync::Once;

use anyhow::{Context, Result, bail};
use llama_cpp_sys_2 as sys;

static BACKEND_INIT: Once = Once::new();

pub struct LlamaEmbeddingRuntime {
    model: *mut sys::llama_model,
    context: *mut sys::llama_context,
    vocab: *const sys::llama_vocab,
    embedding_size: usize,
    vocabulary_size: usize,
    eos_token: i32,
}

unsafe impl Send for LlamaEmbeddingRuntime {}

impl LlamaEmbeddingRuntime {
    pub fn load(path: &Path, context_size: u32, gpu_layers: i32) -> Result<Self> {
        BACKEND_INIT.call_once(|| unsafe {
            sys::llama_backend_init();
        });
        let path = CString::new(path.to_string_lossy().as_bytes())?;
        let mut model_params = unsafe { sys::llama_model_default_params() };
        model_params.n_gpu_layers = gpu_layers;
        let model = unsafe { sys::llama_model_load_from_file(path.as_ptr(), model_params) };
        if model.is_null() {
            bail!("failed to load GGUF model");
        }

        let mut context_params = unsafe { sys::llama_context_default_params() };
        context_params.n_ctx = context_size;
        context_params.n_batch = context_size.min(4096);
        context_params.n_ubatch = context_size.min(512);
        context_params.n_threads = available_threads();
        context_params.n_threads_batch = available_threads();
        let context = unsafe { sys::llama_init_from_model(model, context_params) };
        if context.is_null() {
            unsafe {
                sys::llama_model_free(model);
            }
            bail!("failed to create llama.cpp context");
        }

        let vocab = unsafe { sys::llama_model_get_vocab(model) };
        let embedding_size = unsafe { sys::llama_model_n_embd(model) } as usize;
        let vocabulary_size = unsafe { sys::llama_vocab_n_tokens(vocab) } as usize;
        let eos_token = unsafe { sys::llama_vocab_eos(vocab) };
        Ok(Self {
            model,
            context,
            vocab,
            embedding_size,
            vocabulary_size,
            eos_token,
        })
    }

    pub fn embedding_size(&self) -> usize {
        self.embedding_size
    }

    pub fn tokenize(&self, text: &str, parse_special: bool) -> Result<Vec<i32>> {
        let text = CString::new(text)?;
        let mut tokens = vec![0_i32; text.as_bytes().len().saturating_add(32)];
        let mut count = unsafe {
            sys::llama_tokenize(
                self.vocab,
                text.as_ptr(),
                text.as_bytes().len() as i32,
                tokens.as_mut_ptr(),
                tokens.len() as i32,
                false,
                parse_special,
            )
        };
        if count < 0 {
            tokens.resize((-count) as usize, 0);
            count = unsafe {
                sys::llama_tokenize(
                    self.vocab,
                    text.as_ptr(),
                    text.as_bytes().len() as i32,
                    tokens.as_mut_ptr(),
                    tokens.len() as i32,
                    false,
                    parse_special,
                )
            };
        }
        if count < 0 {
            bail!("llama.cpp tokenization failed");
        }
        tokens.truncate(count as usize);
        Ok(tokens)
    }

    pub fn clear(&mut self) {
        unsafe {
            sys::llama_memory_clear(sys::llama_get_memory(self.context), true);
        }
    }

    pub fn decode_tokens(
        &mut self,
        tokens: &[i32],
        start_position: i32,
        request_logits: bool,
    ) -> Result<()> {
        if tokens.is_empty() {
            return Ok(());
        }
        let mut batch = unsafe { sys::llama_batch_init(tokens.len() as i32, 0, 1) };
        for (index, token) in tokens.iter().enumerate() {
            unsafe {
                *batch.token.add(index) = *token;
                *batch.pos.add(index) = start_position + index as i32;
                *batch.n_seq_id.add(index) = 1;
                *(*batch.seq_id.add(index)) = 0;
                *batch.logits.add(index) = i8::from(request_logits && index + 1 == tokens.len());
            }
        }
        batch.n_tokens = tokens.len() as i32;
        let result = unsafe { sys::llama_decode(self.context, batch) };
        unsafe {
            sys::llama_batch_free(batch);
        }
        if result != 0 {
            bail!("llama.cpp token decode failed with code {result}");
        }
        Ok(())
    }

    pub fn decode_embeddings(
        &mut self,
        embeddings: &[f32],
        token_count: usize,
        positions: &[i32],
        request_logits: bool,
    ) -> Result<()> {
        if embeddings.len() != token_count * self.embedding_size {
            bail!("embedding shape does not match GGUF model");
        }
        if positions.len() != token_count && positions.len() != token_count * 4 {
            bail!("position count must be N or 4N");
        }

        let mut batch =
            unsafe { sys::llama_batch_init(token_count as i32, self.embedding_size as i32, 1) };
        unsafe {
            ptr::copy_nonoverlapping(embeddings.as_ptr(), batch.embd, embeddings.len());
        }
        let allocated_positions = batch.pos;
        if positions.len() == token_count * 4 {
            batch.pos = positions.as_ptr().cast_mut();
        } else {
            unsafe {
                ptr::copy_nonoverlapping(positions.as_ptr(), batch.pos, positions.len());
            }
        }
        for index in 0..token_count {
            unsafe {
                *batch.n_seq_id.add(index) = 1;
                *(*batch.seq_id.add(index)) = 0;
                *batch.logits.add(index) = i8::from(request_logits && index + 1 == token_count);
            }
        }
        batch.n_tokens = token_count as i32;
        let result = unsafe { sys::llama_decode(self.context, batch) };
        batch.pos = allocated_positions;
        unsafe {
            sys::llama_batch_free(batch);
        }
        if result != 0 {
            bail!("llama.cpp embedding decode failed with code {result}");
        }
        Ok(())
    }

    pub fn generate_greedy(
        &mut self,
        start_position: i32,
        maximum_tokens: usize,
        stop_tokens: &[i32],
    ) -> Result<String> {
        let mut output = Vec::new();
        for position in (start_position..).take(maximum_tokens) {
            let logits = unsafe { sys::llama_get_logits(self.context) };
            if logits.is_null() {
                bail!("llama.cpp returned null logits");
            }
            let logits = unsafe { std::slice::from_raw_parts(logits, self.vocabulary_size) };
            let token = logits
                .iter()
                .enumerate()
                .max_by(|left, right| left.1.total_cmp(right.1))
                .map(|(index, _)| index as i32)
                .context("empty llama.cpp vocabulary")?;
            if token == self.eos_token || stop_tokens.contains(&token) {
                break;
            }
            output.push(token);
            self.decode_tokens(&[token], position, true)?;
        }
        self.detokenize(&output)
    }

    pub fn detokenize(&self, tokens: &[i32]) -> Result<String> {
        let mut bytes = Vec::new();
        for token in tokens {
            let mut buffer = vec![0_i8; 256];
            let mut length = unsafe {
                sys::llama_token_to_piece(
                    self.vocab,
                    *token,
                    buffer.as_mut_ptr(),
                    buffer.len() as i32,
                    0,
                    true,
                )
            };
            if length < 0 {
                buffer.resize((-length) as usize, 0);
                length = unsafe {
                    sys::llama_token_to_piece(
                        self.vocab,
                        *token,
                        buffer.as_mut_ptr(),
                        buffer.len() as i32,
                        0,
                        true,
                    )
                };
            }
            if length < 0 {
                bail!("failed to decode llama.cpp token {token}");
            }
            bytes.extend(buffer[..length as usize].iter().map(|byte| *byte as u8));
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

impl Drop for LlamaEmbeddingRuntime {
    fn drop(&mut self) {
        unsafe {
            sys::llama_free(self.context);
            sys::llama_model_free(self.model);
        }
    }
}

fn available_threads() -> i32 {
    std::thread::available_parallelism()
        .map(|count| count.get().min(8) as i32)
        .unwrap_or(4)
}

pub fn gpu_layer_count() -> i32 {
    #[cfg(any(feature = "llama-vulkan", feature = "llama-cuda", feature = "llama-rocm"))]
    {
        -1
    }
    #[cfg(not(any(feature = "llama-vulkan", feature = "llama-cuda", feature = "llama-rocm")))]
    {
        0
    }
}
