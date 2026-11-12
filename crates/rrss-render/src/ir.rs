//! 块 IR。serde tag 用短名，控制 `blocks_json` 体积。

use serde::{Deserialize, Serialize};

pub const IR_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub v: u32,
    pub blocks: Vec<Block>,
}

impl Document {
    pub fn new(blocks: Vec<Block>) -> Self {
        Self {
            v: IR_VERSION,
            blocks,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Block {
    #[serde(rename = "h")]
    Heading { level: u8, inlines: Vec<Inline> },
    #[serde(rename = "p")]
    Paragraph { inlines: Vec<Inline> },
    #[serde(rename = "li")]
    ListItem {
        indent: u8,
        marker: String,
        inlines: Vec<Inline>,
    },
    #[serde(rename = "pre")]
    CodeBlock { lang: Option<String>, text: String },
    #[serde(rename = "img")]
    Image { src: String, alt: String },
    #[serde(rename = "q")]
    Quote { inlines: Vec<Inline> },
    #[serde(rename = "table")]
    Table {
        align: Vec<String>,
        head: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    #[serde(rename = "embed")]
    Embed {
        kind: EmbedKind,
        url: String,
        title: String,
    },
    #[serde(rename = "hr")]
    Rule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbedKind {
    Youtube,
    Bilibili,
    Video,
    Audio,
    Iframe,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Inline {
    #[serde(rename = "t")]
    Text { text: String },
    #[serde(rename = "s")]
    Strong { children: Vec<Inline> },
    #[serde(rename = "e")]
    Emphasis { children: Vec<Inline> },
    #[serde(rename = "c")]
    Code { text: String },
    #[serde(rename = "a")]
    Link { href: String, children: Vec<Inline> },
    #[serde(rename = "br")]
    HardBreak,
}

impl Inline {
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text { text: s.into() }
    }
}
