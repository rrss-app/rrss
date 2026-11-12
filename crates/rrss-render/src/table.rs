use crate::{Block, Inline};

pub(crate) const MAX_TABLE_COLUMNS: usize = 16;
/// 数据行上限；表头另占一行。
pub(crate) const MAX_TABLE_ROWS: usize = 200;

#[derive(Default)]
pub(crate) struct Table {
    pub align: Vec<String>,
    pub head: Vec<Vec<Inline>>,
    pub rows: Vec<Vec<Vec<Inline>>>,
}

impl Table {
    pub fn append_to(mut self, blocks: &mut Vec<Block>) {
        let columns = self.head.len();
        let rows = self.rows.len();
        let width = columns.min(MAX_TABLE_COLUMNS);
        self.head.truncate(width);
        self.align.resize(width, String::new());
        self.rows.truncate(MAX_TABLE_ROWS);
        for row in &mut self.rows {
            row.resize(width, Vec::new());
        }
        blocks.push(Block::Table {
            align: self.align,
            head: self.head,
            rows: self.rows,
        });
        let mut limits = Vec::new();
        if rows > MAX_TABLE_ROWS {
            limits.push(format!("{MAX_TABLE_ROWS} 行"));
        }
        if columns > MAX_TABLE_COLUMNS {
            limits.push(format!("{MAX_TABLE_COLUMNS} 列"));
        }
        if !limits.is_empty() {
            blocks.push(Block::Paragraph {
                inlines: vec![Inline::text(format!(
                    "（表格共 {rows} 行 {columns} 列，只显示前 {}）",
                    limits.join(" ")
                ))],
            });
        }
    }
}
