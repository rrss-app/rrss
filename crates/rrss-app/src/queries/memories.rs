use rusqlite::Connection;

use rrss_core::interests;
use rrss_core::memories;

use crate::views::*;

pub fn memory_page(conn: &Connection, query: &str) -> rrss_core::Result<MemoryPage> {
    let rows = memories::list_memories(conn, query)?
        .into_iter()
        .map(|m| {
            let pending = m.pending();
            MemoryView {
                id: m.id,
                kind: m.kind.as_str().into(),
                content: m.content,
                pinned: m.pinned,
                pending,
                scope: m.scope,
                strength: format!("{:.1}", m.strength),
            }
        })
        .collect();
    let interests = interests::list_interests(conn)?
        .into_iter()
        .map(|item| InterestView {
            id: item.id,
            label: item.label,
            weight: item.weight,
            enabled: item.enabled,
            has_embedding: item.has_embedding,
            hit_count: item.hit_count,
        })
        .collect();
    let (preview, chars) = memories::inject_preview(conn).unwrap_or_else(|_| (String::new(), 0));
    let inject_preview = if preview.is_empty() {
        "注入预览：空".into()
    } else {
        format!("注入预览（{chars} / 900 字）\n{preview}")
    };
    Ok(MemoryPage {
        memories: rows,
        interests,
        inject_preview,
    })
}
