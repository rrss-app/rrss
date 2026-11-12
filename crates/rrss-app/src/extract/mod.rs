//! Offline article extraction; every candidate is sanitized through rrss-render.
mod judge;
mod nav;
pub(crate) mod site_rules;

use rrss_core::db::site_rules::SiteRule;

use dom_query::Document as Dom;
use rrss_render::{Block, Document, RenderOpts};
use url::Url;

pub use judge::Suspicion;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Rule,
    General,
    Fallback,
    Container,
}

#[derive(Debug)]
pub struct Extracted {
    pub title: String,
    pub byline: Option<String>,
    pub published_time: Option<String>,
    pub layer: Layer,
    /// Empty means the result is not suspicious; otherwise contains every reason.
    pub suspicious: Vec<Suspicion>,
    pub document: Document,
    pub text_chars: usize,
    pub image_count: usize,
    pub stripped_elements: usize,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExtractError {
    #[error("no article text")]
    Empty,
    #[error("page is not an article")]
    NotArticle,
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn plain_chars(html: &str, opts: &RenderOpts) -> usize {
    rrss_render::document_to_plain(&rrss_render::html_to_document(html, opts))
        .trim()
        .chars()
        .count()
}

#[derive(Default)]
struct RuleMetadata {
    title: Option<String>,
    author: Option<String>,
    date: Option<String>,
}

struct Page {
    base: Url,
    title: String,
    article_chars: usize,
    metadata: RuleMetadata,
    stripped_elements: usize,
}

impl Page {
    fn fallback(&self, html: &str) -> Option<Extracted> {
        libreadability::extract(html, Some(self.base.as_str()))
            .ok()
            .map(|a| {
                self.candidate(
                    &a.content,
                    a.title,
                    nonempty(a.byline),
                    nonempty(a.published_time),
                    Layer::Fallback,
                )
            })
    }

    fn candidate(
        &self,
        html: &str,
        title: String,
        byline: Option<String>,
        published_time: Option<String>,
        layer: Layer,
    ) -> Extracted {
        let title = self
            .metadata
            .title
            .clone()
            .or_else(|| nonempty(title))
            .unwrap_or_else(|| self.title.clone());
        let byline = self.metadata.author.clone().or(byline);
        let published_time = self.metadata.date.clone().or(published_time);
        let document = rrss_render::html_to_document(
            html,
            &RenderOpts {
                base_url: Some(self.base.clone()),
            },
        );
        let text_chars = rrss_render::document_to_plain(&document)
            .trim()
            .chars()
            .count();
        let image_count = document
            .blocks
            .iter()
            .filter(|b| matches!(b, Block::Image { .. }))
            .count();
        let suspicious = judge::judge(
            text_chars,
            document.blocks.len(),
            self.article_chars,
            &title,
        );
        Extracted {
            title,
            byline,
            published_time,
            layer,
            suspicious,
            document,
            text_chars,
            image_count,
            stripped_elements: self.stripped_elements,
        }
    }
}

/// Consume lazily: don't invoke the next extractor when the current result is good.
fn choose(candidates: impl IntoIterator<Item = Option<Extracted>>) -> Option<Extracted> {
    let mut best: Option<Extracted> = None;
    let mut title = None;
    let mut byline = None;
    let mut published_time = None;
    for mut candidate in candidates.into_iter().flatten() {
        if candidate.layer == Layer::Container {
            if let Some(title) = title.take() {
                candidate.title = title;
                candidate
                    .suspicious
                    .retain(|reason| *reason != Suspicion::MissingTitle);
            }
            candidate.byline = candidate.byline.or_else(|| byline.take());
            candidate.published_time = candidate.published_time.or_else(|| published_time.take());
        } else {
            title = title.or_else(|| nonempty(candidate.title.clone()));
            byline = byline.or_else(|| candidate.byline.clone());
            published_time = published_time.or_else(|| candidate.published_time.clone());
        }
        if candidate.suspicious.is_empty() {
            return Some(candidate);
        }
        if best
            .as_ref()
            .is_none_or(|b| candidate.text_chars > b.text_chars)
        {
            best = Some(candidate);
        }
    }
    best
}

fn normalize_images(dom: &Dom) {
    // Normalize lazy images for all layers, including the container rescue.
    for image in dom.select("img").iter() {
        if let Some(src) = ["data-src", "data-original"]
            .iter()
            .find_map(|name| image.attr(name).filter(|v| !v.trim().is_empty()))
        {
            image.set_attr("src", &src);
        }
    }
}

pub fn extract_article(
    html: &str,
    page_url: &Url,
    rule: Option<&SiteRule>,
) -> Result<Extracted, ExtractError> {
    let dom = Dom::from(html);
    // Select once so overlapping/nested strip selectors count each selected element once.
    let strip = rule
        .map(|r| {
            r.strip
                .iter()
                .filter(|s| !s.trim().is_empty() && dom.try_select(s).is_some())
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let stripped_elements = if strip.is_empty() {
        0
    } else {
        let selected = dom.select(&strip);
        let count = selected.length();
        selected.remove();
        count
    };
    let metadata_text = |selector: &Option<String>| {
        selector
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .and_then(|s| dom.try_select(s))
            .and_then(|selection| {
                selection
                    .iter()
                    .find_map(|node| nonempty(node.text().trim().to_string()))
            })
    };
    let metadata = rule
        .map(|r| RuleMetadata {
            title: metadata_text(&r.title),
            author: metadata_text(&r.author),
            date: metadata_text(&r.date),
        })
        .unwrap_or_default();
    let base = dom
        .select("base[href]")
        .attr("href")
        .and_then(|href| page_url.join(&href).ok())
        .unwrap_or_else(|| page_url.clone());
    let opts = RenderOpts {
        base_url: Some(base.clone()),
    };
    let page = Page {
        base,
        metadata,
        stripped_elements,
        title: dom.select("title").text().trim().to_string(),
        // Largest article avoids counting nested <article> elements twice.
        article_chars: dom
            .select("article")
            .iter()
            .map(|node| plain_chars(&node.html(), &opts))
            .max()
            .unwrap_or(0),
    };
    normalize_images(&dom);
    let normalized = dom.html().to_string();
    if let Some(body) = rule
        .and_then(|r| r.body.as_deref())
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| dom.try_select(s))
    {
        // Keep all disjoint body matches, without duplicating nested selections.
        let nodes = body.nodes();
        let html = nodes
            .iter()
            .filter(|node| {
                !node
                    .ancestors(None)
                    .iter()
                    .any(|parent| nodes.iter().any(|n| n.id == parent.id))
            })
            .map(|node| node.html().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        // Read library metadata only; do not run another body extractor after a rule hit.
        let metadata =
            dom_smoothie::Readability::new(normalized.as_str(), Some(page_url.as_str()), None)
                .ok()
                .map(|reader| reader.get_article_metadata(reader.parse_json_ld()))
                .unwrap_or_default();
        let candidate = page.candidate(
            &html,
            metadata.title,
            metadata.byline,
            metadata.published_time,
            Layer::Rule,
        );
        if candidate.text_chars > 0 {
            return Ok(candidate);
        }
    }
    // Navigation cleanup is a copy: rule hits, article length, and listing
    // evidence stay on the original page.
    let cleaned_dom = dom.clone();
    let removed = nav::strip_navigation(&cleaned_dom);
    tracing::debug!(removed, "stripped navigation");
    let cleaned = cleaned_dom.html().to_string();
    let result = choose((0..3).map(|layer| {
        match layer {
            0 => dom_smoothie::Readability::new(cleaned.as_str(), Some(page_url.as_str()), None)
                .and_then(|mut reader| reader.parse())
                .ok()
                .map(|a| {
                    page.candidate(
                        &a.content,
                        a.title,
                        a.byline,
                        a.published_time,
                        Layer::General,
                    )
                }),
            1 => page.fallback(&cleaned),
            _ => cleaned_dom
                .select("article, main")
                .iter()
                .map(|node| {
                    page.candidate(&node.html(), String::new(), None, None, Layer::Container)
                })
                .max_by_key(|c| c.text_chars),
        }
    }));
    // Compute page evidence independently from what the extractors retained.
    dom.select("script, style, template, noscript").remove();
    let body = dom.select("body");
    let body_chars = body.text().trim().chars().count();
    let link_chars = body
        .select("a")
        .iter()
        .map(|a| a.text().trim().chars().count())
        .sum();
    let items = body.select("li");
    let linked_items = items
        .iter()
        .filter(|li| li.select("a").length() > 0)
        .count();
    let suspicious = result.as_ref().is_none_or(|r| !r.suspicious.is_empty());
    if judge::not_article(
        suspicious,
        body_chars,
        link_chars,
        items.length(),
        linked_items,
    ) {
        return Err(ExtractError::NotArticle);
    }
    result
        .filter(|r| r.text_chars > 0)
        .ok_or(ExtractError::Empty)
}

#[cfg(test)]
mod tests;
