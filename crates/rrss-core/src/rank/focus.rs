//! 阶段 23「M2 离线回放」「接入方式」：焦点查询与回放共用的排序分。

/// 成员筛选保留既有规则分 + 可选兴趣分，不含学来的 B1 加分。
pub const MEMBER_SQL: &str = "(a.score + CASE WHEN ?3 THEN a.interest_boost ELSE 0 END)";

/// SQL 与 Rust 使用相同的规则分 + 可选兴趣分 + B1 加分。
pub const SCORE_SQL: &str =
    "(a.score + CASE WHEN ?3 THEN a.interest_boost ELSE 0 END + COALESCE(r.boost, 0))";

pub fn score(rule_score: i64, interest_boost: i64, boost_on: bool, rank_boost: i64) -> i64 {
    rule_score + if boost_on { interest_boost } else { 0 } + rank_boost
}

#[cfg(test)]
mod tests {
    #[test]
    fn sql_and_memory_score_agree() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let sql = format!(
            "SELECT {} FROM (SELECT ?1 AS score, ?2 AS interest_boost) a LEFT JOIN (SELECT ?4 AS boost) r ON true",
            super::SCORE_SQL
        );
        for (rule, boost, on, rank) in [
            (-10, 12, true, Some(4)),
            (0, 5, false, Some(-2)),
            (3, 7, true, None),
        ] {
            let actual: i64 = conn
                .query_row(&sql, rusqlite::params![rule, boost, on, rank], |r| r.get(0))
                .unwrap();
            assert_eq!(actual, super::score(rule, boost, on, rank.unwrap_or(0)));
        }
    }
}
