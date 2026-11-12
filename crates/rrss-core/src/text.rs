//! 通用文本工具。日志 target：`rrss_core::text`。

/// 按字符数截断，不切断多字节字符。
pub fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => s[..i].to_string(),
        None => s.to_string(),
    }
}
