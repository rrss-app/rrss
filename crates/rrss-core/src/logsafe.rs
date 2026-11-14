//! 共用凭据清洗与日志投影。内容本身（正文、查询、模型响应）应在日志调用处省略。
use std::{fmt, ops::Range, path::Path, sync::OnceLock};

use regex::Regex;
use url::Url;

pub struct Secret<T>(pub T);

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}
impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// 当前 span 已带同名字段时返回空：tracing 不输出空的可选字段，同一行不重复。
/// 不在 span 里（测试、示例、span 被过滤）时照常返回值。
pub fn unless_in_span<T>(field: &str, value: T) -> Option<T> {
    tracing::Span::current()
        .field(field)
        .is_none()
        .then_some(value)
}

pub fn host(value: &Url) -> &str {
    value.host_str().unwrap_or("unknown")
}

pub fn host_str(value: &str) -> String {
    Url::parse(value).map_or_else(|_| "unknown".into(), |value| host(&value).into())
}

pub fn url(value: &str) -> String {
    let Ok(mut value) = Url::parse(value) else {
        return "<invalid url>".into();
    };
    if !matches!(value.scheme(), "http" | "https" | "socks5" | "socks5h") {
        return "<url>".into();
    }
    let _ = value.set_username("");
    let _ = value.set_password(None);
    value.set_query(None);
    value.set_fragment(None);
    oneline(value.as_str(), 200)
}

fn urls() -> &'static Regex {
    static URLS: OnceLock<Regex> = OnceLock::new();
    URLS.get_or_init(|| Regex::new(r#"(?i)\b(?:https?|socks5h?)://[^\\\s<>"'\)\}\]]+"#).unwrap())
}

/// 清洗常见凭据样式；保留换行与诊断内容，不截断、不投影路径。
/// 不是任意正文的脱敏器；操作已知的密钥应传给 `scrub_secrets_with`。
pub fn scrub_secrets(value: &str) -> String {
    scrub_secrets_with(value, std::iter::empty::<&str>())
}

/// 同时清洗已知凭据（包括短值、JSON 转义值）与常见样式。
/// 所有匹配基于原文，合并重叠区间后一次替换，避免短密钥破坏其他凭据的识别。
/// 少于 4 个字符的已知值只在独立出现时匹配（见 `keep_standalone`），避免误伤状态码与诊断单词。
pub fn scrub_secrets_with<'a>(value: &str, secrets: impl IntoIterator<Item = &'a str>) -> String {
    static KEYS: OnceLock<Regex> = OnceLock::new();
    static TOKENS: OnceLock<Regex> = OnceLock::new();
    static HEADERS: OnceLock<Regex> = OnceLock::new();
    let keys = KEYS.get_or_init(|| Regex::new(r#"(?i)\b(?:(?:proxy[-_])?authorization|api[-_]?key|access[-_]?token|refresh[-_]?token|token|password|passwd|secret|set[-_]?cookie|cookie|key)["']?\s*[:=]\s*("(?:\\.|[^"\\])*"|'[^']*'|[^\s,;\}\]]+)"#).unwrap());
    let tokens = TOKENS.get_or_init(|| {
        Regex::new(r#"(?i)\b(?:sk-[a-z0-9_-]+|Bearer\s+[^\s,"'\)\}\]]+|Basic\s+[a-z0-9+/=]+)"#)
            .unwrap()
    });
    let headers = HEADERS.get_or_init(|| {
        Regex::new(r"(?im)\b(?:(?:proxy[-_])?authorization|set-cookie|cookie)[ \t]*[:=]([^\r\n]*)")
            .unwrap()
    });
    let mut ranges = Vec::new();
    for pattern in [keys, headers] {
        ranges.extend(
            pattern
                .captures_iter(value)
                .map(|c| c.get(1).unwrap().range()),
        );
    }
    ranges.extend(tokens.find_iter(value).map(|m| m.range()));
    for found in urls().find_iter(value) {
        let raw = found.as_str();
        if Url::parse(raw).is_err() {
            ranges.push(found.range());
            continue;
        }
        let authority_start = raw.find("://").unwrap() + 3;
        let authority = raw[authority_start..]
            .split(['/', '?', '#'])
            .next()
            .unwrap();
        if let Some(at) = authority.rfind('@') {
            ranges.push(found.start() + authority_start..found.start() + authority_start + at);
        }
        if let Some(query) = raw.find(['?', '#']) {
            // 查询与片段整体隐藏，不猜测其中哪些参数是凭据。
            ranges.push(found.start() + query + 1..found.end());
        }
    }
    let mut short = Vec::new();
    for secret in secrets.into_iter().filter(|secret| !secret.is_empty()) {
        let found = if secret.chars().count() >= 4 {
            &mut ranges
        } else {
            &mut short
        };
        literal_ranges(value, secret, found);
        let escaped = serde_json::to_string(secret).expect("string serializes");
        let escaped = &escaped[1..escaped.len() - 1];
        if escaped != secret {
            literal_ranges(value, escaped, found);
        }
    }
    // 已有掩码也视作一个整体；重复处理或密钥本身包含星号时不增长掩码。
    literal_ranges(value, "***", &mut ranges);
    keep_standalone(value, short, &mut ranges);
    ranges.retain(|range| !range.is_empty());
    ranges.sort_unstable_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for mut range in ranges {
        if merged.last().is_some_and(|last| range.end <= last.end) {
            continue;
        }
        while range.start > 0 && value.as_bytes()[range.start - 1] == b'*' {
            range.start -= 1;
        }
        while range.end < value.len() && value.as_bytes()[range.end] == b'*' {
            range.end += 1;
        }
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    let mut out = String::with_capacity(value.len());
    let mut end = 0;
    for range in merged {
        out.push_str(&value[end..range.start]);
        out.push_str("***");
        end = range.end;
    }
    out.push_str(&value[end..]);
    out
}

fn literal_ranges(value: &str, needle: &str, ranges: &mut Vec<Range<usize>>) {
    let mut start = 0;
    while let Some(offset) = value[start..].find(needle) {
        let at = start + offset;
        ranges.push(at..at + needle.len());
        // 允许重叠匹配，例如密钥 ababa 出现在 abababa 中。
        start = at + value[at..].chars().next().unwrap().len_utf8();
    }
}

/// 短值只保留独立出现：两侧紧邻字符不是字母或数字，或已被其他命中遮住。
/// 后者让一次清洗与再清洗一遍一致（第二遍时那一侧已是 `***`）；新接受的短值可能让别的短值成立，所以重复到不再变化。
fn keep_standalone(value: &str, mut pending: Vec<Range<usize>>, ranges: &mut Vec<Range<usize>>) {
    fn bounded(ranges: &[Range<usize>], side: Option<char>, at: usize, before: bool) -> bool {
        let Some(c) = side else { return true };
        let char_range = if before {
            at - c.len_utf8()..at
        } else {
            at..at + c.len_utf8()
        };
        !c.is_alphanumeric()
            || ranges
                .iter()
                .any(|r| r.start <= char_range.start && char_range.end <= r.end)
    }
    loop {
        let before = pending.len();
        pending.retain(|range| {
            let left = value[..range.start].chars().next_back();
            let right = value[range.end..].chars().next();
            let standalone = bounded(ranges, left, range.start, true)
                && bounded(ranges, right, range.end, false);
            if standalone {
                ranges.push(range.clone());
            }
            !standalone
        });
        if pending.len() == before {
            break;
        }
    }
}

fn roots() -> &'static Vec<(String, &'static str)> {
    static ROOTS: OnceLock<Vec<(String, &'static str)>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        let mut roots = Vec::new();
        if let Ok(value) = crate::paths::log_dir() {
            roots.push((value.to_string_lossy().into_owned(), "<logs>"));
        }
        if let Ok(value) = crate::paths::data_dir() {
            roots.push((value.to_string_lossy().into_owned(), "<data>"));
        }
        if let Some(dirs) = directories::BaseDirs::new() {
            roots.push((dirs.home_dir().to_string_lossy().into_owned(), "~"));
        }
        roots.sort_by_key(|(root, _)| std::cmp::Reverse(root.len()));
        roots
    })
}

pub fn path(value: &Path) -> String {
    text(&value.to_string_lossy(), 300)
}

pub fn error(value: &crate::Error) -> String {
    match value {
        crate::Error::Http { status, .. } | crate::Error::Network { status, .. } => {
            format!("http status {status}")
        }
        crate::Error::Parse(_) => "feed parse failed".into(),
        crate::Error::InvalidMemory(_) => "invalid memory".into(),
        crate::Error::InvalidRule(_) => "invalid rule".into(),
        // Sentence-style validation failures are still legacy UI text. Logging
        // their type avoids copying display Chinese or arbitrary input/output.
        crate::Error::InvalidProvider(_) => "invalid AI provider".into(),
        crate::Error::InvalidSetting(_) => "invalid setting".into(),
        crate::Error::InvalidNetworkProxy(_) => "invalid network proxy".into(),
        crate::Error::InvalidPiOverride(_) => "invalid Pi override".into(),
        crate::Error::InvalidModelChoice(_) => "invalid model choice".into(),
        crate::Error::InvalidTreeOrder(_) => "invalid tree order".into(),
        crate::Error::InvalidCompat(_) => "invalid Pi compat object".into(),
        crate::Error::Backup(_) => "backup operation failed".into(),
        crate::Error::SchemaTooOld { found, min } => {
            format!("schema version {found} is too old (minimum: {min})")
        }
        crate::Error::VectorSpaceRebuildRequired { .. } => "vector space rebuild required".into(),
        crate::Error::VectorSpaceRebuildInProgress => "vector space rebuild in progress".into(),
        crate::Error::TaskModelUnset(_) => "task model is unset".into(),
        crate::Error::FetchDeferred { .. } => "fetch deferred".into(),
        crate::Error::FetchPaused(_) => "fetch paused".into(),
        _ => text(&value.to_string(), 500),
    }
}

/// 错误、panic、外部 stderr 等不可信字段的最后一道文本处理。
pub fn text(value: &str, max: usize) -> String {
    let value = scrub_secrets(value);
    let mut value = urls()
        .replace_all(&value, |caps: &regex::Captures<'_>| url(&caps[0]))
        .into_owned();
    for (root, replacement) in roots() {
        value = value.replace(root, replacement);
        value = value.replace(&root.replace('\\', "\\\\"), replacement);
    }
    // 同时处理来自另一台机器的错误与导出内容；不读取该机器的用户信息。
    static HOMES: OnceLock<Regex> = OnceLock::new();
    let homes = HOMES.get_or_init(|| {
        Regex::new(r"(?i)(?:/home/|/Users/)[^/\s\\]+|[a-z]:\\(?:\\)?Users\\(?:\\)?[^\\\s]+")
            .unwrap()
    });
    oneline(&homes.replace_all(&value, "~"), max)
}

/// 以输出字符数封顶，转义控制字符，不能伪造新日志行。
pub fn oneline(value: &str, max: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    for c in value.chars() {
        let escaped = match c {
            '\n' => "\\n".into(),
            '\r' => "\\r".into(),
            '\t' => "\\t".into(),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') => {
                c.escape_unicode().to_string()
            }
            c => c.to_string(),
        };
        let size = escaped.chars().count();
        if count + size > max {
            out.push('…');
            break;
        }
        out.push_str(&escaped);
        count += size;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_credentials_are_cleaned_without_flattening_diagnostics() {
        let raw = concat!(
            "HTTP 401\nproxy-authorization: bEaReR bearer-canary\n",
            "authorization: bAsIc Y2FuYXJ5\n",
            "aUtHoRiZaTiOn: DeepL-Auth-Key deepl-canary\n",
            "{\"accessToken\":\"access-canary\",\"refresh_token\":\"refresh-canary\",",
            "\"apiKey\":\"api-canary\",\"password\":\"password-canary\"}\n",
            "https://user-canary:pass-canary@example.test/v1?custom=query-canary#fragment-canary\n",
            "socks5h://socks-canary:pw-canary@proxy.test:1080\n",
            "sk-plain-canary\n529 overloaded",
        );
        let clean = scrub_secrets(raw);
        assert!(!clean.contains("canary") && !clean.contains("Y2FuYXJ5"));
        assert!(clean.starts_with("HTTP 401\n"));
        assert!(clean.ends_with("\n529 overloaded"));
        assert!(clean.contains("example.test/v1") && clean.contains("proxy.test:1080"));
        assert_eq!(scrub_secrets(&clean), clean);
    }

    #[test]
    fn known_raw_and_json_escaped_values_are_cleaned_together() {
        let secret = "短\\值\"\n\t";
        let raw = format!(
            "rejected {secret}\n{}\nnext diagnostic",
            serde_json::json!({"detail": secret})
        );
        let clean = scrub_secrets_with(&raw, [secret, ""]);
        assert!(!clean.contains(secret) && !clean.contains("短"));
        assert!(clean.starts_with("rejected ***\n") && clean.ends_with("\nnext diagnostic"));
        assert_eq!(scrub_secrets_with(&clean, [secret]), clean);
    }

    #[test]
    fn overlapping_and_short_values_do_not_break_other_matches() {
        for (raw, secrets, expected) in [
            (
                "a ab abc ababa",
                vec!["a", "ab", "abc", "aba"],
                "*** *** *** ababa",
            ),
            ("failed: xy; 中", vec!["", "xy", "中"], "failed: ***; ***"),
            ("password=long-value", vec!["pass"], "***word=***"),
            ("Bearer abc def", vec!["abc def"], "***"),
            ("mask: *** ** *", vec!["**", "*"], "mask: *** *** ***"),
            ("abababa", vec!["ababa"], "***"),
            ("HTTP 500; 0", vec!["0"], "HTTP 500; ***"),
            (
                "token expired; authentication required; en",
                vec!["en"],
                "token expired; authentication required; ***",
            ),
            (
                "中文; 中; éen; １1; en界",
                vec!["中", "en", "1"],
                "中文; ***; éen; １1; en界",
            ),
            (r#"a\"; a\"z; a""#, vec!["a\""], r#"***; a\"z; ***"#),
            // 短值紧挨长值或另一个已遮住的短值时，那一侧算边界；互相依赖的粘连短值都保留。
            ("value=wxyzq end", vec!["wxyz", "q"], "value=*** end"),
            ("wxyzq-r; wxyzqr", vec!["wxyz", "q", "r"], "***-***; ***qr"),
        ] {
            let clean = scrub_secrets_with(raw, secrets.iter().copied());
            assert_eq!(clean, expected);
            assert_eq!(scrub_secrets_with(&clean, secrets.iter().copied()), clean);
        }
    }

    #[test]
    fn short_known_values_only_match_as_standalone_tokens() {
        let clean = scrub_secrets_with(
            "HTTP 401 Unauthorized, retry in 10s; value 1; token en; en",
            ["1", "en"],
        );
        assert_eq!(
            clean,
            "HTTP 401 Unauthorized, retry in 10s; value ***; token ***; ***"
        );
    }

    #[test]
    fn only_log_projection_limits_urls_and_escapes_control_characters() {
        let path = "诊断".repeat(150);
        let raw = format!("https://user:pw@example.test/{path}?token=canary\nnext\tline");
        let clean = scrub_secrets(&raw);
        assert!(clean.contains(&path) && clean.contains("\nnext\tline"));
        let log = text(&clean, 1000);
        assert!(!log.contains(&path) && !log.contains("canary") && !log.contains("user:"));
        assert!(!log.contains('?') && !log.contains('\n') && !log.contains('\t'));
        // 清洗先于截断：截断位置不能留下已知密钥的前半段。
        let clean = scrub_secrets_with(
            "error arbitrary-private-value suffix",
            ["arbitrary-private-value"],
        );
        assert_eq!(text(&clean, 12), "error *** su…");
    }

    #[test]
    fn credentials_urls_and_control_characters_do_not_survive() {
        let value = text(
            "https://alice:fake-password@example.test/feed?token=canary-query#secret\nAuthorization: Bearer canary-bearer\n{\"api_key\":\"canary-key\"} sk-canary-key",
            2000,
        );
        for secret in [
            "alice",
            "fake-password",
            "canary-query",
            "canary-bearer",
            "canary-key",
            "\n",
        ] {
            assert!(!value.contains(secret), "leaked {secret}");
        }
        assert!(value.contains("example.test/feed"));
        assert_eq!(
            format!("{} {:?}", Secret("fake-key"), Secret("fake-key")),
            "*** ***"
        );
        assert_eq!(oneline("中\n文abc", 5), "中\\n文a…");
    }

    #[test]
    fn span_fields_are_not_repeated_on_events() {
        let capture = crate::test_log::Capture::new();
        capture.run(|| {
            tracing::info!(feed_id = unless_in_span("feed_id", 7), "outside");
            let span = tracing::warn_span!("feed", feed_id = 7);
            span.in_scope(|| tracing::info!(feed_id = unless_in_span("feed_id", 7), "inside"));
        });
        let text = capture.take();
        let line = |needle: &str| {
            text.lines()
                .find(|line| line.contains(needle))
                .unwrap()
                .to_owned()
        };
        assert_eq!(line("outside").matches("feed_id=7").count(), 1, "{text}");
        assert_eq!(line("inside").matches("feed_id=7").count(), 1, "{text}");
    }

    #[test]
    fn paths_from_both_platforms_and_malformed_urls_are_safe() {
        assert_eq!(text("/home/example/private/file", 200), "~/private/file");
        assert_eq!(
            text(r"C:\Users\example\private\file", 200),
            r"~\private\file"
        );
        assert_eq!(url("bad?token=canary"), "<invalid url>");
        assert_eq!(
            url("https://u:p@example.test/a?q=s#f"),
            "https://example.test/a"
        );
    }

    #[test]
    fn json_credentials_and_multivalue_cookies_are_redacted() {
        let value = scrub_secrets(
            r#"{"Cookie":"first=canary-cookie; second=canary-cookie-two", "password":"canary-password", "Authorization":"Basic Y2FuYXJ5"}"#,
        );
        for secret in ["canary-cookie", "canary-password", "Y2FuYXJ5"] {
            assert!(!value.contains(secret));
        }
        assert!(!scrub_secrets("Cookie: a=canary-one; b=canary-two").contains("canary"));
        let once = text("https://user:pass@example.test/a?key=canary\nnext", 200);
        assert_eq!(text(&once, 200), once);
    }
}
