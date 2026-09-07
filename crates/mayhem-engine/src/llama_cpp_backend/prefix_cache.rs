use crate::{EngineError, Result};
use llama_cpp_2::{
    context::{LlamaContext, session::LlamaStateSeqFlags},
    token::LlamaToken,
};

/// One bounded, private prompt snapshot, stored in RAM on Linux providers.
/// It never contains sampled output or state from another loaded model/config.
#[derive(Debug)]
pub(super) struct PrefixCache {
    max_bytes: usize,
    tokens: Vec<LlamaToken>,
    state: Option<tempfile::NamedTempFile>,
    last: (usize, usize),
}

impl PrefixCache {
    pub(super) fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            tokens: Vec::new(),
            state: None,
            last: (0, 0),
        }
    }

    pub(super) fn from_env() -> Result<Self> {
        let max_bytes = match std::env::var("MAYHEM_LLM_PREFIX_CACHE_MAX_BYTES") {
            Ok(value) => value.parse::<usize>().map_err(|_| {
                EngineError::InvalidConfig(
                    "MAYHEM_LLM_PREFIX_CACHE_MAX_BYTES must be an unsigned byte count".into(),
                )
            })?,
            Err(_) => 8 * 1024 * 1024 * 1024,
        };
        if max_bytes < 64 * 1024 * 1024 {
            return Err(EngineError::InvalidConfig(
                "prefix caching is required; MAYHEM_LLM_PREFIX_CACHE_MAX_BYTES must be at least 64 MiB".into(),
            ));
        }
        Ok(Self::new(max_bytes))
    }

    pub(super) fn enabled(&self) -> bool {
        self.max_bytes >= 64 * 1024 * 1024
    }

    pub(super) fn clear(&mut self) {
        self.tokens = Vec::new();
        self.state = None;
        self.last = (0, 0);
    }

    pub(super) fn last_tokens(&self) -> (usize, usize) {
        self.last
    }

    pub(super) fn restore(
        &mut self,
        ctx: &mut LlamaContext<'_>,
        prompt: &[LlamaToken],
    ) -> Result<usize> {
        // Always decode at least the final token to produce logits for this
        // request. Samplers and output decoding state are never reused.
        let common = self
            .tokens
            .iter()
            .zip(prompt)
            .take_while(|(a, b)| a == b)
            .count()
            .min(prompt.len().saturating_sub(1));
        self.last = (prompt.len(), 0);
        if common == 0 || self.state.is_none() {
            return Ok(0);
        }
        let file = self.state.as_ref().expect("snapshot present");
        let (tokens, _) = ctx
            .state_seq_load_file(file.path(), 0, self.tokens.len())
            .map_err(|e| {
                EngineError::InvalidConfig(format!("llama.cpp prefix restore failed: {e}"))
            })?;
        if tokens != self.tokens {
            self.clear();
            return Err(EngineError::InvalidConfig(
                "llama.cpp prefix token identity changed".into(),
            ));
        }
        let trimmed = common == self.tokens.len()
            || ctx
                .clear_kv_cache_seq(Some(0), Some(common as u32), None)
                .map_err(|e| {
                    EngineError::InvalidConfig(format!("llama.cpp prefix trim failed: {e}"))
                })?;
        if !trimmed {
            // Recurrent/SWA backends may not support a requested rollback.
            // An unsupported prefix is a miss; discard all restored state.
            ctx.clear_kv_cache();
            return Ok(0);
        }
        self.last.1 = common;
        Ok(common)
    }

    pub(super) fn save(&mut self, ctx: &LlamaContext<'_>, prompt: &[LlamaToken]) -> Result<()> {
        if self.max_bytes == 0 {
            return Ok(());
        }
        let flags = LlamaStateSeqFlags::empty();
        let size = ctx.state_seq_get_size_ext(0, flags);
        let token_bytes = std::mem::size_of_val(prompt);
        // Release the old allocation before creating the replacement, keeping
        // peak snapshot memory within the operator's configured bound.
        self.state = None;
        self.tokens = Vec::new();
        if size == 0 || size.saturating_add(token_bytes) > self.max_bytes {
            eprintln!(
                "prefix_cache_store backend=llama.cpp prompt_tokens={} cached_tokens={} stored_bytes=0 limit_bytes={}",
                self.last.0, self.last.1, self.max_bytes
            );
            return Ok(());
        }
        // Linux providers retain snapshots on tmpfs, never their model disk.
        // The temporary file is private (0600) and removed when replaced/dropped.
        #[cfg(target_os = "linux")]
        let directory = std::path::PathBuf::from("/dev/shm");
        #[cfg(not(target_os = "linux"))]
        let directory = std::env::temp_dir();
        let state = tempfile::Builder::new()
            .prefix("mayhem-prefix-")
            .tempfile_in(directory)
            .map_err(|e| {
                EngineError::InvalidConfig(format!("llama.cpp prefix cache storage: {e}"))
            })?;
        let copied = ctx
            .state_seq_save_file(state.path(), 0, prompt)
            .map_err(|e| {
                EngineError::InvalidConfig(format!("llama.cpp prefix capture failed: {e}"))
            })?;
        if copied > self.max_bytes {
            return Ok(());
        }
        self.tokens = prompt.to_vec();
        self.state = Some(state);
        eprintln!(
            "prefix_cache_store backend=llama.cpp prompt_tokens={} cached_tokens={} stored_bytes={}",
            self.last.0, self.last.1, size
        );
        Ok(())
    }
}
