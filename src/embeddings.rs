use candle_core::{Device, Tensor, DType};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use tokenizers::Tokenizer;
use std::path::PathBuf;

pub struct LocalPredictor {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl LocalPredictor {
    pub fn init_from_disk() -> anyhow::Result<Self> {
        let device = Device::Cpu;

        let model_dir = std::env::var("STACK_INTERCEPT_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let exe = std::env::current_exe().ok()
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                    .unwrap_or_else(|| PathBuf::from("."));
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

        let config_bytes = std::fs::read_to_string(config_path)?;
        let config: Config = serde_json::from_str(&config_bytes)?;

        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Tokenizer read issue: {}", e))?;

        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], DType::F32, &device)?
        };

        let model = BertModel::load(vb, &config)?;

        Ok(Self { model, tokenizer, device })
    }

    pub fn encode_text(&self, raw_prompt: &str) -> anyhow::Result<Vec<f32>> {
        let tokens = self.tokenizer.encode(raw_prompt, true)
            .map_err(|e| anyhow::anyhow!("Tokenization fail: {}", e))?;

        let ids = tokens.get_ids();
        let id_tensor = Tensor::new(ids, &self.device)?.unsqueeze(0)?;
        let token_type_ids = id_tensor.zeros_like()?;

        let hidden_states = self.model.forward(&id_tensor, &token_type_ids, None)?;

        // Mean pooling across sequence dimension
        let (_batch, sequence_len, _dim) = hidden_states.dims3()?;
        let pooled = (hidden_states.sum(1)? / (sequence_len as f64))?.squeeze(0)?;

        // L2 normalization
        let structural_norm = pooled.sqr()?.sum(0)?.sqrt()?;
        let normalized = pooled.broadcast_div(&structural_norm)?;

        Ok(normalized.to_vec1::<f32>()?)
    }
}
