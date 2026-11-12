use serde::Deserialize;

#[derive(Deserialize)]
pub struct ChatCompletion {
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Deserialize)]
pub struct Choice {
    pub message: ChoiceMessage,
}

#[derive(Deserialize)]
pub struct ChoiceMessage {
    pub content: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: Option<u32>,
    #[serde(default)]
    pub completion_tokens: Option<u32>,
}

#[derive(Deserialize)]
pub struct ResponsesEnvelope {
    #[serde(default)]
    pub output: Vec<ResponsesItem>,
    #[serde(default)]
    pub usage: Option<ResponsesUsage>,
}

#[derive(Deserialize)]
pub struct ResponsesItem {
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub content: Vec<ResponsesContent>,
}

#[derive(Deserialize)]
pub struct ResponsesContent {
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct ResponsesUsage {
    #[serde(default)]
    pub input_tokens: Option<u32>,
    #[serde(default)]
    pub output_tokens: Option<u32>,
}

#[derive(Deserialize)]
pub struct ApiErrorBody {
    pub error: ApiError,
}

#[derive(Deserialize)]
pub struct ApiError {
    pub message: Option<String>,
}
