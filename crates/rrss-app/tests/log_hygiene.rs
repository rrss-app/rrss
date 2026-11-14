//! 扫描 Rust 语法中的日志宏，不把注释、文档样例或字符串当调用。
use proc_macro2::TokenTree;
use std::{
    fs,
    path::{Path, PathBuf},
};
use syn::visit::Visit;

fn source_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            source_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

struct Check {
    failures: Vec<String>,
}
impl<'ast> Visit<'ast> for Check {
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let path: Vec<_> = node
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        if path.first().map(String::as_str) != Some("tracing") {
            return;
        }
        let level = path.last().unwrap().as_str();
        if !["info", "warn", "error", "debug", "trace"].contains(&level) {
            return;
        }
        let tokens: Vec<_> = node.tokens.clone().into_iter().collect();
        let raw = node.tokens.to_string();
        let label = raw.chars().take(180).collect::<String>();
        for window in tokens.windows(2) {
            if matches!(&window[0], TokenTree::Ident(name) if name == "target")
                && matches!(&window[1], TokenTree::Punct(p) if p.as_char() == ':')
                && !raw.contains("rrss::panic")
                && !raw.contains("rrss_ui")
            {
                self.failures.push(format!("explicit target: {label}"));
            }
        }
        for token in &tokens {
            if let TokenTree::Literal(value) = token
                && let Ok(text) = syn::parse_str::<syn::LitStr>(&value.to_string())
                && text
                    .value()
                    .chars()
                    .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
            {
                self.failures.push(format!("non-English literal: {label}"));
            }
        }
        let fields: Vec<_> = tokens
            .split(|t| matches!(t, TokenTree::Punct(p) if p.as_char() == ','))
            .filter_map(|arg| {
                arg.iter().find_map(|token| {
                    if let TokenTree::Ident(name) = token {
                        Some(name.to_string())
                    } else {
                        None
                    }
                })
            })
            .collect();
        // UI 错误的原因是 `code`（加 `detail`）；其余警告仍用 `error` 或 `error_kind`。
        if level == "warn"
            && !fields
                .iter()
                .any(|name| name == "error" || name == "error_kind" || name == "code")
        {
            self.failures
                .push(format!("warning without reason: {label}"));
        }
        if matches!(level, "info" | "warn")
            && fields
                .iter()
                .any(|f| ["url", "referer", "body", "path"].contains(&f.as_str()))
            && !raw.contains("logsafe")
        {
            self.failures.push(format!("unsafe field: {label}"));
        }
    }
}

#[test]
fn logging_events_follow_the_privacy_and_field_contract() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    for entry in fs::read_dir(crates).unwrap() {
        let src = entry.unwrap().path().join("src");
        if src.is_dir() {
            source_files(&src, &mut files);
        }
    }
    let mut failures = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        let parsed = syn::parse_file(&text).unwrap();
        let mut check = Check {
            failures: Vec::new(),
        };
        check.visit_file(&parsed);
        failures.extend(
            check
                .failures
                .into_iter()
                .map(|error| format!("{}: {error}", file.strip_prefix(crates).unwrap().display())),
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
