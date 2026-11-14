//! Display projection for pre-existing runtime display assertions after typed transport.
//! Uses the shipped translations; new error tests assert codes instead.
pub fn display(error: &impl serde::Serialize) -> String {
    let value = serde_json::to_value(error).unwrap();
    let translations: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../apps/desktop/src/locales/zh-CN/errors.json"
    ))
    .unwrap();
    let code = value["code"].as_str().unwrap();
    let mut output = translations[code].as_str().unwrap().to_string();
    let detail = if value["cause"].is_object() {
        display(&value["cause"])
    } else {
        value["detail"].as_str().unwrap_or_default().to_string()
    };
    output = output.replace("{{detail}}", &detail);
    if code == "mcp_process_exit" || code == "mcp_process_killed" {
        let ms = value["params"]["elapsed_ms"].as_u64().unwrap();
        output = output.replace("{{seconds}}", &format!("{:.1}", ms as f64 / 1000.0));
    }
    if let Some(params) = value["params"].as_object() {
        for (key, val) in params {
            let text = if code == "pi_download_failed" && key == "stage" {
                translations["pi_download_stage"][val.as_str().unwrap()]
                    .as_str()
                    .unwrap()
                    .to_string()
            } else if code == "pi_download_failed" && key == "message" {
                translations["network_message"][val.as_str().unwrap()]
                    .as_str()
                    .unwrap()
                    .to_string()
            } else if code.starts_with("chat_handshake_") && key == "step" {
                translations["chat_handshake_step"][val.as_str().unwrap()]
                    .as_str()
                    .unwrap()
                    .to_string()
            } else if code == "chat_model_mismatch" && key == "actual" && val.is_null() {
                translations["chat_model_unselected"]
                    .as_str()
                    .unwrap()
                    .to_string()
            } else {
                val.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| val.to_string())
            };
            output = output.replace(&format!("{{{{{key}}}}}"), &text);
        }
    }
    output
}

pub fn proxy_env(error: &impl serde::Serialize) -> String {
    let translations: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../apps/desktop/src/locales/zh-CN/errors.json"
    ))
    .unwrap();
    translations["pi_proxy_env_failed"]
        .as_str()
        .unwrap()
        .replace("{{error}}", &display(error))
}

pub fn probe_error(result: &impl serde::Serialize) -> String {
    let value = serde_json::to_value(result).unwrap();
    if value["failure"].is_object() {
        display(&value["failure"])
    } else {
        value["error"].as_str().unwrap_or_default().to_string()
    }
}
pub fn login_error(result: &impl serde::Serialize) -> String {
    let value = serde_json::to_value(result).unwrap();
    if value["error"].is_object() {
        display(&value["error"])
    } else {
        value["message"].as_str().unwrap_or_default().to_string()
    }
}
pub fn probe_output(result: &impl serde::Serialize) -> String {
    let value = serde_json::to_value(result).unwrap();
    if value["output_omitted"] == true {
        let translations: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../apps/desktop/src/locales/zh-CN/errors.json"
        ))
        .unwrap();
        translations["mcp_output_omitted"]
            .as_str()
            .unwrap()
            .to_string()
    } else {
        value["output"].as_str().unwrap_or_default().to_string()
    }
}
