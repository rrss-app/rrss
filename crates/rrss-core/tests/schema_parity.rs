//! Compare a fresh database with a v33 snapshot upgraded to SCHEMA_VERSION.
//! Do not whitelist differences: a failing parity test is a migration defect to
//! resolve explicitly, including legacy tables intentionally retained for data.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;
use rrss_core::Error;
use rrss_core::db::{MIN_UPGRADE_VERSION, SCHEMA_VERSION, migrations::migrate, open_at};
use rusqlite::{Connection, OptionalExtension, Result};
use serde_json::{Value, json};

const CURRENT: &str = include_str!("../src/schema.sql");
const V33: &str = include_str!("../../../testdata/schema/v33.sql");
type Snapshot = BTreeMap<String, Value>;

// SQLite does not expose predicates, trigger bodies, CHECKs or virtual module
// arguments through PRAGMA. Tokenize just those SQL fragments: ignore comments,
// whitespace and keyword/identifier case, but preserve string literal contents.
// This is lexical normalization, not an algebraic SQL equivalence prover.
fn tokens(sql: &str) -> Vec<String> {
    static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?s)--[^\n]*(?:\n|$)|/\*.*?\*/|'(?:''|[^'])*'|"(?:""|[^"])*"|`(?:``|[^`])*`|\[[^\]]*\]|[\p{L}_][\p{L}\p{N}_$]*|\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|<=|>=|<>|!=|==|\|\||->>|->|[^\s]"#).unwrap()
    });
    let mut out: Vec<String> = TOKEN
        .find_iter(sql)
        .filter_map(|m| {
            let s = m.as_str();
            if s.starts_with("--") || s.starts_with("/*") {
                None
            } else if s.starts_with('\'') {
                Some(s.to_owned())
            } else if s.starts_with('"') || s.starts_with('`') || s.starts_with('[') {
                let inner = &s[1..s.len() - 1];
                Some(
                    inner
                        .replace("\"\"", "\"")
                        .replace("``", "`")
                        .to_lowercase(),
                )
            } else {
                Some(s.to_lowercase())
            }
        })
        .collect();
    while out.last().is_some_and(|t| t == ";") {
        out.pop();
    }
    out
}

fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

// Return a balanced parenthesized fragment and the first position after it.
fn group(ts: &[String], start: usize) -> (Vec<String>, usize) {
    assert_eq!(ts[start], "(");
    let mut depth = 1;
    for end in start + 1..ts.len() {
        match ts[end].as_str() {
            "(" => depth += 1,
            ")" => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return (ts[start + 1..end].to_vec(), end + 1);
        }
    }
    panic!("unbalanced schema fragment: {ts:?}");
}

fn comma_parts(ts: &[String]) -> Vec<Vec<String>> {
    let mut parts = vec![];
    let mut start = 0;
    let mut depth = 0;
    for (i, t) in ts.iter().enumerate() {
        match t.as_str() {
            "(" => depth += 1,
            ")" => depth -= 1,
            "," if depth == 0 => {
                parts.push(ts[start..i].to_vec());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(ts[start..].to_vec());
    parts
}

fn table_sql_details(out: &mut Snapshot, table: &str, sql: &str) {
    let ts = tokens(sql);
    let prefix = format!("table {table}");
    let Some(open) = ts.iter().position(|t| t == "(") else {
        return;
    };
    let (body, end) = group(&ts, open);
    if ts.get(1).is_some_and(|t| t == "virtual") {
        let using = ts.iter().position(|t| t == "using").unwrap();
        out.insert(format!("{prefix}/virtual module"), json!(ts[using + 1]));
        let mut args = comma_parts(&body);
        args.sort(); // declared column order is not a parity requirement
        out.insert(format!("{prefix}/virtual arguments"), json!(args));
        return;
    }
    out.insert(format!("{prefix}/options"), json!(&ts[end..]));
    let mut checks = vec![];
    for part in comma_parts(&body) {
        for (i, t) in part.iter().enumerate() {
            if t == "check" && part.get(i + 1).is_some_and(|t| t == "(") {
                checks.push(group(&part, i + 1).0);
            }
            if t == "autoincrement" {
                out.insert(
                    format!("{prefix}/column {}/autoincrement", part[0]),
                    json!(true),
                );
            }
            if t == "collate" {
                out.insert(
                    format!("{prefix}/column {}/collation", part[0]),
                    json!(part[i + 1]),
                );
            }
            if t == "as" && part.get(i + 1).is_some_and(|t| t == "(") {
                out.insert(
                    format!("{prefix}/column {}/generated expression", part[0]),
                    json!(group(&part, i + 1).0),
                );
            }
        }
    }
    checks.sort();
    out.insert(format!("{prefix}/checks"), json!(checks));
}

fn snapshot(conn: &Connection) -> Result<Snapshot> {
    let mut out = Snapshot::new();
    let objects = conn
        .prepare("SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name")?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>>>()?;
    for (kind, name, table, sql) in objects {
        // sqlite_sequence is allocator state, not a user table; AUTOINCREMENT is
        // compared separately. Autoindexes are inspected through index_list.
        if name.starts_with("sqlite_") || kind == "index" {
            continue;
        }
        let prefix = format!("{kind} {name}");
        out.insert(prefix.clone(), json!({"table": table}));
        if kind != "table" {
            out.insert(
                format!("{prefix}/definition"),
                json!(tokens(sql.as_deref().unwrap())),
            );
            continue;
        }
        table_sql_details(&mut out, &name, sql.as_deref().unwrap());
        // xinfo extends table_info with hidden/generated virtual-table columns.
        let mut stmt = conn.prepare(&format!("PRAGMA table_xinfo({})", ident(&name)))?;
        let columns = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?;
        for column in columns {
            let (column, ty, not_null, default, pk, hidden) = column?;
            let key = format!("{prefix}/column {column}");
            out.insert(key.clone(), json!(true));
            for (field, value) in [
                ("type", json!(tokens(&ty))),
                ("not null", json!(not_null)),
                ("default", json!(default.map(|s| tokens(&s)))),
                ("primary key position", json!(pk)),
                ("hidden", json!(hidden)),
            ] {
                out.insert(format!("{key}/{field}"), value);
            }
        }
        let foreign_keys = conn.prepare(&format!("PRAGMA foreign_key_list({})", ident(&name)))?
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, json!({"table": r.get::<_, String>(2)?, "from": r.get::<_, String>(3)?, "to": r.get::<_, Option<String>>(4)?, "on update": r.get::<_, String>(5)?, "on delete": r.get::<_, String>(6)?, "match": r.get::<_, String>(7)?}))))?
            .collect::<Result<Vec<_>>>()?;
        let mut grouped: BTreeMap<i64, BTreeMap<i64, Value>> = BTreeMap::new();
        for (id, seq, fk) in foreign_keys {
            grouped.entry(id).or_default().insert(seq, fk);
        }
        let mut foreign_keys: Vec<_> = grouped
            .into_values()
            .map(|v| v.into_values().collect::<Vec<_>>())
            .collect();
        foreign_keys.sort_by_key(|v| serde_json::to_string(v).unwrap());
        out.insert(format!("{prefix}/foreign keys"), json!(foreign_keys));

        let indices = conn
            .prepare(&format!("PRAGMA index_list({})", ident(&name)))?
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, bool>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>>>()?;
        for (index, unique, origin, partial) in indices {
            let columns = conn
                .prepare(&format!("PRAGMA index_xinfo({})", ident(&index)))?
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, bool>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, bool>(5)?,
                    ))
                })?
                .collect::<Result<Vec<_>>>()?;
            // WITHOUT ROWID primary-key indexes can appear in index_list
            // without a separate sqlite_master row.
            let sql: Option<String> = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE name=?1",
                    [&index],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            assert!(
                origin != "c" || sql.is_some(),
                "missing SQL for index {index}"
            );
            let ts = sql.as_deref().map(tokens).unwrap_or_default();
            let terms = ts
                .iter()
                .position(|t| t == "(")
                .map(|i| comma_parts(&group(&ts, i).0))
                .unwrap_or_default();
            let mut keys = vec![];
            for (cid, column, desc, collation, key) in columns {
                if !key {
                    continue;
                } // physical covering columns may follow table order
                let expression = (cid == -2).then(|| terms[keys.len()].clone());
                keys.push(json!({"column":column, "expression":expression, "rowid":cid == -1, "descending":desc, "collation":collation}));
            }
            let label = if origin == "c" {
                index
            } else {
                format!("{origin}:{}", serde_json::to_string(&keys).unwrap())
            };
            let prefix = format!("table {name}/index {label}");
            out.insert(prefix.clone(), json!(true));
            out.insert(format!("{prefix}/unique"), json!(unique));
            out.insert(format!("{prefix}/origin"), json!(origin));
            out.insert(format!("{prefix}/columns"), json!(keys));
            out.insert(format!("{prefix}/partial"), json!(partial));
            let predicate = ts
                .iter()
                .position(|t| t == "where")
                .map(|i| ts[i + 1..].to_vec());
            out.insert(format!("{prefix}/predicate"), json!(predicate));
        }
    }
    Ok(out)
}

fn settings(conn: &Connection) -> Result<Snapshot> {
    conn.prepare("SELECT key,value FROM settings ORDER BY key")?
        .query_map([], |r| {
            Ok((
                format!("settings {}", r.get::<_, String>(0)?),
                json!(r.get::<_, String>(1)?),
            ))
        })?
        .collect()
}

fn differences(expected: &Snapshot, actual: &Snapshot) -> Vec<String> {
    expected
        .keys()
        .chain(actual.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|key| expected.get(*key) != actual.get(*key))
        // A missing whole table/column/index gets one diagnostic; details of
        // existing objects still identify every changed property.
        .filter(|key| {
            !key.match_indices('/').any(|(i, _)| {
                let parent = &key[..i];
                expected.contains_key(parent) != actual.contains_key(parent)
            })
        })
        .map(|key| {
            format!(
                "{key}: expected {}, actual {}",
                expected
                    .get(key)
                    .map(Value::to_string)
                    .unwrap_or_else(|| "<missing>".into()),
                actual
                    .get(key)
                    .map(Value::to_string)
                    .unwrap_or_else(|| "<missing>".into())
            )
        })
        .collect()
}

#[test]
fn v33_upgrade_matches_fresh_database() {
    let dir = tempfile::tempdir().unwrap();
    // Use the real pool bootstrap for v0 (also registers sqlite-vec).
    let fresh = open_at(&dir.path().join("fresh.db")).unwrap();
    let declared = Connection::open_in_memory().unwrap();
    declared.execute_batch(CURRENT).unwrap();
    let defaults = settings(&declared).unwrap();
    let upgraded = Connection::open_in_memory().unwrap();
    upgraded.pragma_update(None, "foreign_keys", true).unwrap();
    upgraded.execute_batch(V33).unwrap();
    upgraded
        .pragma_update(None, "user_version", MIN_UPGRADE_VERSION)
        .unwrap();
    migrate(&upgraded).unwrap();
    let fresh_settings = settings(&fresh).unwrap();
    let mut upgraded_settings = settings(&upgraded).unwrap();
    let mut failures = vec![];
    for diff in differences(&snapshot(&fresh).unwrap(), &snapshot(&upgraded).unwrap()) {
        failures.push(format!("structure: {diff}"));
    }
    for (key, value) in &defaults {
        if fresh_settings.get(key) != Some(value) {
            failures.push(format!(
                "fresh schema.sql default {key}: expected {value}, actual {:?}",
                fresh_settings.get(key)
            ));
        }
        if upgraded_settings.get(key) != Some(value) {
            failures.push(format!(
                "v33 schema.sql default {key}: expected {value}, actual {:?}",
                upgraded_settings.get(key)
            ));
        }
    }
    // v40 删除旧库预写的预算 0，新旧库均由注册表提供默认值；仅升级提示不同。
    assert_eq!(
        upgraded_settings.remove("settings focus_auto_summary_notice"),
        Some(json!("default"))
    );
    for diff in differences(&fresh_settings, &upgraded_settings) {
        failures.push(format!("settings: {diff}"));
    }
    for (label, conn) in [("fresh", &*fresh), ("v33", &upgraded)] {
        let version = conn
            .pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap();
        if version != SCHEMA_VERSION {
            failures.push(format!("{label} user_version {version}"));
        }
    }
    assert!(
        failures.is_empty(),
        "schema parity failures (fresh vs v33 upgraded to v{SCHEMA_VERSION}):\n{}",
        failures.join("\n")
    );
}

#[test]
fn schema_older_than_minimum_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    {
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", MIN_UPGRADE_VERSION - 1)
            .unwrap();
    }
    let err = match open_at(&path) {
        Err(err) => err,
        Ok(_) => panic!("expected SchemaTooOld"),
    };
    let text = err.to_string();
    assert!(
        matches!(
            err,
            Error::SchemaTooOld {
                found: 32,
                min: MIN_UPGRADE_VERSION
            }
        ),
        "{text}"
    );
    assert_eq!(
        text,
        "数据库版本 v32 过旧，最低支持 v33：请先用旧版本打开一次完成升级，或新建数据库"
    );
}

#[test]
fn comparison_ignores_formatting_and_table_column_order() {
    let a = Connection::open_in_memory().unwrap();
    let b = Connection::open_in_memory().unwrap();
    a.execute_batch(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT NOT NULL DEFAULT 'A B');
        CREATE UNIQUE INDEX idx ON t(name DESC) WHERE id > 0;
        CREATE TRIGGER tr AFTER INSERT ON t BEGIN UPDATE t SET name='A B' WHERE id=new.id; END;",
    )
    .unwrap();
    b.execute_batch(
        "create table \"t\"(name text not null default 'A B',id integer primary key);
        create unique index idx on t ( name desc ) where id>0;
        create trigger tr after insert on t begin -- spacing only
        update t set name = 'A B' where id = new.id; end;",
    )
    .unwrap();
    assert!(differences(&snapshot(&a).unwrap(), &snapshot(&b).unwrap()).is_empty());
}

#[test]
fn comparison_detects_structural_and_virtual_table_changes() {
    let dir = tempfile::tempdir().unwrap();
    let _registered = open_at(&dir.path().join("register-vec.db")).unwrap();
    let a = Connection::open_in_memory().unwrap();
    let b = Connection::open_in_memory().unwrap();
    for (conn, sql) in [
        (
            &a,
            "CREATE TABLE t(id INTEGER PRIMARY KEY, value TEXT NOT NULL DEFAULT 'A B');
        CREATE UNIQUE INDEX idx ON t(value DESC) WHERE id>0;
        CREATE TRIGGER tr AFTER INSERT ON t BEGIN UPDATE t SET value='A B'; END;
        CREATE VIRTUAL TABLE f USING fts5(body, tokenize='trigram');
        CREATE VIRTUAL TABLE v USING vec0(embedding float[2]);",
        ),
        (
            &b,
            "CREATE TABLE t(id INTEGER, value BLOB DEFAULT 'AB');
        CREATE INDEX idx ON t(value) WHERE id>1;
        CREATE TRIGGER tr AFTER INSERT ON t BEGIN UPDATE t SET value='AB'; END;
        CREATE VIRTUAL TABLE f USING fts5(body, tokenize='unicode61');
        CREATE VIRTUAL TABLE v USING vec0(embedding float[3]);
        CREATE TABLE extra(id INTEGER);",
        ),
    ] {
        conn.execute_batch(sql).unwrap();
    }
    let diff = differences(&snapshot(&a).unwrap(), &snapshot(&b).unwrap()).join("\n");
    for path in [
        "table t/column id/primary key position",
        "table t/column value/type",
        "table t/column value/not null",
        "table t/column value/default",
        "table t/index idx/unique",
        "table t/index idx/columns",
        "table t/index idx/predicate",
        "trigger tr/definition",
        "table f/virtual arguments",
        "table v/virtual arguments",
        "table extra",
    ] {
        assert!(diff.contains(path), "missing diagnostic {path}:\n{diff}");
    }
    assert!(
        !differences(
            &BTreeMap::from([("settings sample".into(), json!("1"))]),
            &BTreeMap::from([("settings sample".into(), json!("0"))])
        )
        .is_empty()
    );
}
