use base64::Engine;
use byteburrow_plugin_api::{
    declare_plugin, ClassificationResult, ClassifierPlugin, FileContext, PluginConfig,
    API_VERSION_MAJOR, API_VERSION_MINOR,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";
const DEFAULT_MODEL: &str = "qwen3.5:9b";
const DEFAULT_TIMEOUT_SECS: u64 = 120;

const PROMPT: &str = "\
Analyze this image and extract descriptive keywords. \
Return ONLY a JSON array of lowercase English keyword strings, nothing else. \
Include keywords for: objects, scene type, colors, mood, activities, setting (indoor/outdoor), \
weather/lighting if visible, and any notable details. \
Example: [\"sunset\",\"beach\",\"ocean\",\"waves\",\"orange sky\",\"silhouette\",\"person\",\"outdoor\"] \
Keep keywords concise (1-3 words each). Return 5-20 keywords.";

pub struct KeywordExtractor {
    ollama_url: String,
    model: String,
    timeout: Duration,
    /// Only one Ollama request in-flight at a time.
    inflight: Mutex<()>,
}

#[derive(Serialize)]
struct OllamaRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    images: Vec<String>,
    stream: bool,
    options: OllamaOptions,
}

#[derive(Serialize)]
struct OllamaOptions {
    temperature: f32,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

impl KeywordExtractor {
    fn new() -> Self {
        Self {
            ollama_url: DEFAULT_OLLAMA_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            inflight: Mutex::new(()),
        }
    }

    fn call_ollama(&self, image_data: &[u8]) -> Result<Vec<String>, String> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(image_data);

        let request = OllamaRequest {
            model: &self.model,
            prompt: PROMPT,
            images: vec![b64],
            stream: false,
            options: OllamaOptions { temperature: 0.3 },
        };

        let url = format!("{}/api/generate", self.ollama_url);

        let agent = ureq::Agent::new_with_config(
            ureq::config::Config::builder()
                .timeout_global(Some(self.timeout))
                .build(),
        );

        let resp: OllamaResponse = agent
            .post(&url)
            .send_json(&request)
            .map_err(|e| format!("Ollama request failed: {e}"))?
            .body_mut()
            .read_json()
            .map_err(|e| format!("Failed to parse Ollama response: {e}"))?;

        parse_keywords(&resp.response)
    }
}

fn parse_keywords(raw: &str) -> Result<Vec<String>, String> {
    // The LLM might wrap the JSON in markdown code fences or add extra text.
    // Try to extract the JSON array from the response.
    let trimmed = raw.trim();

    // Try direct parse first
    if let Ok(keywords) = serde_json::from_str::<Vec<String>>(trimmed) {
        return Ok(clean_keywords(keywords));
    }

    // Try to find a JSON array in the response
    if let Some(start) = trimmed.find('[') {
        if let Some(end) = trimmed.rfind(']') {
            let slice = &trimmed[start..=end];
            if let Ok(keywords) = serde_json::from_str::<Vec<String>>(slice) {
                return Ok(clean_keywords(keywords));
            }
        }
    }

    Err(format!(
        "Could not parse keywords from Ollama response: {trimmed}"
    ))
}

fn clean_keywords(keywords: Vec<String>) -> Vec<String> {
    keywords
        .into_iter()
        .map(|k| k.trim().to_lowercase())
        .filter(|k| !k.is_empty() && k.len() <= 50)
        .collect()
}

impl ClassifierPlugin for KeywordExtractor {
    fn name(&self) -> &str {
        "Keyword Extractor (Ollama)"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn api_version(&self) -> (u32, u32) {
        (API_VERSION_MAJOR, API_VERSION_MINOR)
    }

    fn mime_interests(&self) -> &[&str] {
        &["image/"]
    }

    fn needs_file_data(&self) -> bool {
        true
    }

    fn init(&mut self, config: &PluginConfig) -> Result<(), String> {
        if let Some(url) = config
            .get("ollama_url")
            .or(std::env::var("BYTEBURROW_OLLAMA_URL").ok().as_ref())
        {
            self.ollama_url = url.clone();
        }

        if let Some(model) = config
            .get("ollama_model")
            .or(std::env::var("BYTEBURROW_OLLAMA_MODEL").ok().as_ref())
        {
            self.model = model.clone();
        }

        if let Some(timeout) =
            config
                .get("ollama_timeout")
                .or(std::env::var("BYTEBURROW_OLLAMA_TIMEOUT").ok().as_ref())
        {
            let secs: u64 = timeout
                .parse()
                .map_err(|_| format!("Invalid timeout value: {timeout}"))?;
            self.timeout = Duration::from_secs(secs);
        }

        Ok(())
    }

    fn classify(&self, ctx: &FileContext) -> Result<Option<ClassificationResult>, String> {
        if ctx.data.is_empty() {
            return Ok(None);
        }

        // Serialize Ollama access — only one request in-flight at a time.
        // Other job threads block here until the current request finishes.
        let _guard = self.inflight.lock().unwrap_or_else(|e| e.into_inner());

        let keywords = match self.call_ollama(ctx.data) {
            Ok(kw) if kw.is_empty() => return Ok(None),
            Ok(kw) => kw,
            Err(e) => {
                eprintln!("keyword-extractor: {e}");
                return Ok(None);
            }
        };

        let mut custom = HashMap::new();
        custom.insert(
            "ai_keywords".to_string(),
            serde_json::json!({
                "model": self.model,
                "keywords": &keywords,
            }),
        );

        Ok(Some(ClassificationResult {
            keywords,
            custom,
            ..Default::default()
        }))
    }
}

declare_plugin!(KeywordExtractor::new());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_json_array() {
        let kw = parse_keywords(r#"["sunset","beach","ocean"]"#).unwrap();
        assert_eq!(kw, vec!["sunset", "beach", "ocean"]);
    }

    #[test]
    fn parse_markdown_fenced_array() {
        // LLMs often wrap the JSON in a ```json fence.
        let raw = "Sure! Here are the keywords:\n```json\n[\"cat\",\"dog\"]\n```\n";
        let kw = parse_keywords(raw).unwrap();
        assert_eq!(kw, vec!["cat", "dog"]);
    }

    #[test]
    fn parse_text_wrapped_array() {
        // Prose before and after the array.
        let raw = "The keywords are [\"red\",\"green\",\"blue\"] hope that helps";
        let kw = parse_keywords(raw).unwrap();
        assert_eq!(kw, vec!["red", "green", "blue"]);
    }

    #[test]
    fn parse_unparseable_returns_err() {
        assert!(parse_keywords("just some words, no array here").is_err());
    }

    #[test]
    fn clean_keywords_trims_lowercases_and_caps_length() {
        let input = vec![
            "  SunSet ".to_string(),
            "BEACH".to_string(),
            "".to_string(),
            "x".repeat(60),
        ];
        let cleaned = clean_keywords(input);
        // Empty dropped, over-50-char dropped, rest trimmed+lowercased.
        assert_eq!(cleaned, vec!["sunset", "beach"]);
    }

    #[test]
    fn clean_keywords_keeps_fifty_char_boundary() {
        // Exactly 50 chars is kept; 51 is dropped.
        let kept = "k".repeat(50);
        let dropped = "k".repeat(51);
        let cleaned = clean_keywords(vec![kept.clone(), dropped]);
        assert_eq!(cleaned, vec![kept]);
    }
}
