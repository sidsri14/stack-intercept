use candle_core::{Device, Tensor, DType};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use tokenizers::Tokenizer;
use std::path::PathBuf;

pub struct SemanticEmbedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl SemanticEmbedder {
    pub fn load() -> anyhow::Result<Self> {
        let device = Device::Cpu;

        // Model directory relative to the binary, or overridden by STACK_INTERCEPT_MODEL_DIR env var
        let model_dir = std::env::var("STACK_INTERCEPT_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                // Default: look for model/ relative to the binary's location
                let exe = std::env::current_exe().ok()
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                    .unwrap_or_else(|| PathBuf::from("."));
                // Walk up to find the project root with model/ directory
                let mut path = exe.clone();
                loop {
                    let candidate = path.join("model");
                    if candidate.join("config.json").exists() {
                        return candidate;
                    }
                    if !path.pop() {
                        break;
                    }
                }
                PathBuf::from("model")
            });
        let weights_path = model_dir.join("model.safetensors");
        let tokenizer_path = model_dir.join("tokenizer.json");
        let config_path = model_dir.join("config.json");

        let config_str = std::fs::read_to_string(config_path)?;
        let config: Config = serde_json::from_str(&config_str)?;

        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Tokenizer loading failed: {}", e))?;

        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], DType::F32, &device)?
        };

        let model = BertModel::load(vb, &config)?;

        Ok(Self { model, tokenizer, device })
    }

    pub fn generate_vector(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let tokens = self.tokenizer.encode(text, true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

        let token_ids = tokens.get_ids();
        let token_ids_tensor = Tensor::new(token_ids, &self.device)?.unsqueeze(0)?;
        let token_type_ids = token_ids_tensor.zeros_like()?;

        // Forward pass: shape (1, n_tokens, 384)
        let embeddings = self.model.forward(&token_ids_tensor, &token_type_ids, None)?;

        // Mean pooling across the token sequence dimension
        let (_n_batch, n_tokens, _hidden_size) = embeddings.dims3()?;
        let mean_embedding = (embeddings.sum(1)? / (n_tokens as f64))?;
        let mean_embedding = mean_embedding.squeeze(0)?;

        // L2 normalize so cosine similarity = dot product
        let norm = mean_embedding.sqr()?.sum(0)?.sqrt()?;
        let normalized_tensor = mean_embedding.broadcast_div(&norm)?;

        Ok(normalized_tensor.to_vec1::<f32>()?)
    }
}
