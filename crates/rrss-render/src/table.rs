use crate::{Block, Inline, TableTotal};

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
        let cut = rows > MAX_TABLE_ROWS || columns > MAX_TABLE_COLUMNS;
        blocks.push(Block::Table {
            align: self.align,
            head: self.head,
            rows: self.rows,
            total: cut.then(|| TableTotal {
                rows: u32::try_from(rows).unwrap_or(u32::MAX),
                columns: u32::try_from(columns).unwrap_or(u32::MAX),
            }),
        });
    }
}
