//! Navigation cleanup for one document copy. Class names are ignored.
//! `aside` and `footer` are never removed; recognition libraries still own them.

use std::collections::HashSet;

use dom_query::{Document, NodeId, NodeRef, Selection};

const LANDMARKS: &str = "nav, [role=navigation], [role=menubar], [role=menu], [role=banner]";
const HEADERS: &str = "header";
const LISTS: &str = "ul, ol";
const ITEMS: &str = "li";
const LINKS: &str = "a";
/// Link text at or above this share of a header's text, outside `article`.
const HEADER_LINK_PERCENT: usize = 60;
const MIN_MENU_ITEMS: usize = 5;
/// Share of list items whose own text is a link.
const MENU_ITEM_PERCENT: usize = 80;
/// A list item is "mainly a link" at this share of its own non-whitespace text.
const ITEM_LINK_PERCENT: usize = 80;

/// Remove obvious navigation from `dom` and return how many elements were removed.
pub(super) fn strip_navigation(dom: &Document) -> usize {
    let mut marked = Vec::new();
    for node in dom.select(LANDMARKS).nodes() {
        push_unique(&mut marked, *node);
    }
    for header in dom.select(HEADERS).nodes() {
        if !in_article(header) && header_is_nav(header) {
            push_unique(&mut marked, *header);
        }
    }
    for list in dom.select(LISTS).nodes() {
        if !in_article(list) && is_dropdown(list) {
            push_unique(&mut marked, *list);
        }
    }
    let ids: HashSet<NodeId> = marked.iter().map(|node| node.id).collect();
    let roots: Vec<_> = marked
        .into_iter()
        .filter(|node| {
            !node
                .ancestors(None)
                .iter()
                .any(|ancestor| ids.contains(&ancestor.id))
        })
        .collect();
    for node in &roots {
        node.remove_from_parent();
    }
    roots.len()
}

fn push_unique<'a>(marked: &mut Vec<NodeRef<'a>>, node: NodeRef<'a>) {
    if marked.iter().all(|existing| existing.id != node.id) {
        marked.push(node);
    }
}

fn matches<'a>(node: NodeRef<'a>, selector: &str) -> Vec<NodeRef<'a>> {
    Selection::from(node).select(selector).nodes().to_vec()
}

fn in_article(node: &NodeRef<'_>) -> bool {
    node.ancestors(None)
        .iter()
        .any(|ancestor| ancestor.has_name("article"))
}

fn header_is_nav(header: &NodeRef<'_>) -> bool {
    !matches(*header, LANDMARKS).is_empty()
        || matches(*header, LISTS).iter().any(is_dropdown)
        || percent_at_least(link_chars(header), text_chars(header), HEADER_LINK_PERCENT)
}

fn is_dropdown(list: &NodeRef<'_>) -> bool {
    if matches(*list, LISTS).is_empty() {
        return false;
    }
    let items = matches(*list, ITEMS);
    let linked = items.iter().filter(|item| item_is_link(item)).count();
    items.len() >= MIN_MENU_ITEMS && percent_at_least(linked, items.len(), MENU_ITEM_PERCENT)
}

fn item_is_link(item: &NodeRef<'_>) -> bool {
    let (total, linked) = own_chars(item, false);
    percent_at_least(linked, total, ITEM_LINK_PERCENT)
}

/// Text of `node` excluding nested lists, so a parent item is judged by its label.
fn own_chars(node: &NodeRef<'_>, in_link: bool) -> (usize, usize) {
    if node.has_name("ul") || node.has_name("ol") {
        return (0, 0);
    }
    if node.is_text() {
        let count = significant(&node.text());
        return (count, if in_link { count } else { 0 });
    }
    let in_link = in_link || node.has_name("a");
    node.children()
        .iter()
        .fold((0, 0), |(total, linked), child| {
            let (child_total, child_linked) = own_chars(child, in_link);
            (total + child_total, linked + child_linked)
        })
}

fn link_chars(node: &NodeRef<'_>) -> usize {
    matches(*node, LINKS)
        .iter()
        .map(|anchor| significant(&anchor.text()))
        .sum()
}

fn text_chars(node: &NodeRef<'_>) -> usize {
    significant(&node.text())
}

fn significant(text: &str) -> usize {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .count()
}

fn percent_at_least(part: usize, whole: usize, percent: usize) -> bool {
    whole > 0 && part.saturating_mul(100) >= whole.saturating_mul(percent)
}
