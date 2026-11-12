//! 兼容声明与版本闸。版本范围只来自 `pi/compat.json`，文档与界面不抄写版本号。

use std::sync::OnceLock;

use serde::Deserialize;

use super::adapter::PiVersion;

const COMPAT_JSON: &str = include_str!("../../../../pi/compat.json");

/// `tested`：托管下载安装的精确版本；`min`：低于它拒绝启动；`line`：兼容线（major.minor）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compat {
    pub tested: PiVersion,
    pub min: PiVersion,
    pub line: (u32, u32),
}

#[derive(Deserialize)]
struct Raw {
    tested: String,
    min: String,
    line: String,
}

/// 随应用发布的兼容声明。文件损坏属于构建错误，单测会拦住。
pub fn compat() -> &'static Compat {
    static COMPAT: OnceLock<Compat> = OnceLock::new();
    COMPAT.get_or_init(|| parse(COMPAT_JSON).expect("pi/compat.json"))
}

fn parse(raw: &str) -> Option<Compat> {
    let raw: Raw = serde_json::from_str(raw).ok()?;
    let (major, minor) = raw.line.split_once('.')?;
    Some(Compat {
        tested: PiVersion::parse(&raw.tested)?,
        min: PiVersion::parse(&raw.min)?,
        line: (major.parse().ok()?, minor.parse().ok()?),
    })
}

/// 版本闸的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionGate {
    /// `min` 至 `line` 之内。
    Supported,
    /// 高于兼容线，或读不出版本：允许启动，状态标注「未验证版本」。
    Unverified,
    /// 低于 `min`：拒绝启动。
    TooOld,
}

impl Compat {
    pub fn gate(&self, version: Option<&PiVersion>) -> VersionGate {
        let Some(v) = version else {
            return VersionGate::Unverified;
        };
        if *v < self.min {
            VersionGate::TooOld
        } else if self.in_line(v) {
            VersionGate::Supported
        } else {
            VersionGate::Unverified
        }
    }

    pub fn in_line(&self, v: &PiVersion) -> bool {
        (v.major, v.minor) == self.line
    }

    pub fn line_label(&self) -> String {
        format!("{}.{}", self.line.0, self.line.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(raw: &str) -> PiVersion {
        PiVersion::parse(raw).unwrap()
    }

    #[test]
    fn shipped_declaration_parses_and_is_consistent() {
        let c = compat();
        assert!(c.min <= c.tested);
        assert!(c.in_line(&c.tested) && c.in_line(&c.min));
        assert!(parse(r#"{"tested": "x", "min": "0.1.0", "line": "0.1"}"#).is_none());
    }

    #[test]
    fn gate_follows_min_and_line() {
        let c = parse(r#"{"tested": "0.87.1", "min": "0.87.0", "line": "0.87"}"#).unwrap();
        assert_eq!(c.gate(Some(&v("0.84.4"))), VersionGate::TooOld);
        assert_eq!(c.gate(Some(&v("0.87.0"))), VersionGate::Supported);
        assert_eq!(c.gate(Some(&v("0.87.9"))), VersionGate::Supported);
        assert_eq!(c.gate(Some(&v("0.88.0"))), VersionGate::Unverified);
        assert_eq!(c.gate(None), VersionGate::Unverified);
    }
}
