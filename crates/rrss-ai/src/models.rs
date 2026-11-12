//! OpenAI 兼容 `/v1/models` 及 Ollama `/api/tags` 模型列表拉取。日志 target：`rrss_ai::models`。

use serde::Deserialize;
use serde_json::Value;

use crate::complete::{Error, Result};

#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    data: Option<Vec<OpenAiModelItem>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelItem {
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Option<Vec<OllamaTagItem>>,
}

#[derive(Debug, Deserialize)]
struct OllamaTagItem {
    name: Option<String>,
    model: Option<String>,
}

pub async fn list_models(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<String>> {
    let base = base_url.trim();
    if base.is_empty() {
        return Err(Error::NoProvider);
    }
    let api_key = api_key.map(str::trim).filter(|key| !key.is_empty());
    let urls = candidate_model_urls(base);
    let mut last_err = None;

    for url in urls {
        let mut req = client.get(&url);
        if let Some(key) = api_key {
            req = req.bearer_auth(key);
        }
        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                crate::complete::check_proxy_auth(status)?;
                if status.is_success() {
                    if let Ok(body) = resp.text().await {
                        if let Ok(models) = parse_models_json(&body) {
                            if !models.is_empty() {
                                tracing::debug!(
                                    count = models.len(),
                                    host = %rrss_core::logsafe::host_str(&url),
                                    "fetched models successfully"
                                );
                                return Ok(models);
                            }
                        }
                    }
                } else {
                    let body = resp.text().await.unwrap_or_default();
                    last_err = Some(Error::Http {
                        status: status.as_u16(),
                        message: rrss_core::logsafe::scrub_secrets_with(&body, api_key),
                    });
                }
            }
            Err(err) => {
                last_err = Some(Error::from(err));
            }
        }
    }

    Err(last_err
        .unwrap_or_else(|| Error::InvalidJson("failed to fetch models from any endpoint".into())))
}

pub fn candidate_model_urls(base: &str) -> Vec<String> {
    let trimmed = base.trim().trim_end_matches('/');
    let mut urls = Vec::new();

    if trimmed.ends_with("/models") || trimmed.ends_with("/api/tags") {
        urls.push(trimmed.to_string());
    } else if trimmed.ends_with("/v1") {
        urls.push(format!("{trimmed}/models"));
        let root = trimmed.trim_end_matches("/v1");
        urls.push(format!("{root}/api/tags"));
    } else {
        urls.push(format!("{trimmed}/v1/models"));
        urls.push(format!("{trimmed}/models"));
        urls.push(format!("{trimmed}/api/tags"));
    }
    urls.dedup();
    urls
}

pub fn parse_models_json(body: &str) -> Result<Vec<String>> {
    let mut models = Vec::new();

    // 1. Try OpenAI format: { "data": [ { "id": "model-id" } ] }
    if let Ok(parsed) = serde_json::from_str::<OpenAiModelsResponse>(body) {
        if let Some(items) = parsed.data {
            for item in items {
                if let Some(id) = item
                    .id
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                {
                    models.push(id);
                }
            }
        }
    }

    // 2. Try Ollama format: { "models": [ { "name": "...", "model": "..." } ] }
    if models.is_empty() {
        if let Ok(parsed) = serde_json::from_str::<OllamaTagsResponse>(body) {
            if let Some(items) = parsed.models {
                for item in items {
                    if let Some(name) = item
                        .name
                        .or(item.model)
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                    {
                        models.push(name);
                    }
                }
            }
        }
    }

    // 3. Fallback generic array
    if models.is_empty() {
        if let Ok(val) = serde_json::from_str::<Value>(body) {
            if let Some(arr) = val.as_array() {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        models.push(s.trim().to_string());
                    } else if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                        models.push(id.trim().to_string());
                    } else if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                        models.push(name.trim().to_string());
                    }
                }
            }
        }
    }

    models.sort();
    models.dedup();
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_openai_models() {
        let json = r#"{
            "object": "list",
            "data": [
                {"id": "gpt-4o", "object": "model"},
                {"id": "text-embedding-3-small", "object": "model"}
            ]
        }"#;
        let list = parse_models_json(json).unwrap();
        assert_eq!(list, vec!["gpt-4o", "text-embedding-3-small"]);
    }

    #[test]
    fn parse_ollama_tags() {
        let json = r#"{
            "models": [
                {"name": "llama3.2:latest", "model": "llama3.2:latest"},
                {"name": "nomic-embed-text:latest", "model": "nomic-embed-text:latest"}
            ]
        }"#;
        let list = parse_models_json(json).unwrap();
        assert_eq!(list, vec!["llama3.2:latest", "nomic-embed-text:latest"]);
    }

    #[test]
    fn candidate_urls() {
        assert_eq!(
            candidate_model_urls("http://127.0.0.1:11434/v1"),
            vec![
                "http://127.0.0.1:11434/v1/models",
                "http://127.0.0.1:11434/api/tags"
            ]
        );
        assert_eq!(
            candidate_model_urls("https://api.openai.com/v1"),
            vec![
                "https://api.openai.com/v1/models",
                "https://api.openai.com/api/tags"
            ]
        );
    }
}
