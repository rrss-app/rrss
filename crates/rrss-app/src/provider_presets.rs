//! 连接预设的唯一默认值来源；选择预设只填充草稿，不发请求、不写库。
use crate::views::ProviderPreset;

pub fn provider_presets() -> Vec<ProviderPreset> {
    [
        ("openai", "OpenAI", "https://api.openai.com/v1"),
        ("deepseek", "DeepSeek", "https://api.deepseek.com/v1"),
        ("openrouter", "OpenRouter", "https://openrouter.ai/api/v1"),
        ("ollama", "Ollama（本机）", "http://127.0.0.1:11434/v1"),
        ("openai_compat", "自定义 OpenAI 兼容", ""),
    ]
    .into_iter()
    .map(|(id, name, base_url)| ProviderPreset {
        id: id.into(),
        name: name.into(),
        base_url: base_url.into(),
        protocol: "chat".into(),
        // 沿用运行时兼容默认值；未经探测不猜测供应商能力。
        compat_json: "{}".into(),
    })
    .collect()
}
