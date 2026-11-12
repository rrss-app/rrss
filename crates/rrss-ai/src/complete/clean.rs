use super::{Error, Result};
use serde_json::Value;

pub fn parse_json_content(raw: &str) -> Result<Value> {
    let trimmed = raw.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        if v.is_object() || v.is_array() {
            return Ok(v);
        }
        return Err(Error::InvalidJson(trimmed.into()));
    }
    let start = trimmed.find(['{', '[']);
    let end = trimmed.rfind(['}', ']']);
    if let (Some(s), Some(e)) = (start, end)
        && e >= s
        && let Ok(v) = serde_json::from_str::<Value>(&trimmed[s..=e])
    {
        return Ok(v);
    }
    Err(Error::InvalidJson(trimmed.into()))
}

pub fn strip_thinking(text: &str) -> String {
    let mut cur = text.to_string();
    loop {
        let lower = cur.to_lowercase();
        if let Some(start) = lower.find("<think>") {
            if let Some(end) = lower.find("</think>") {
                if end >= start {
                    let mut next = cur[..start].to_string();
                    next.push_str(&cur[end + "</think>".len()..]);
                    cur = next;
                    continue;
                }
            }
            cur = cur[..start].to_string();
            break;
        }
        break;
    }
    cur.trim().to_string()
}
