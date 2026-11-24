use std::{path::Path, sync::Arc};

use rrss_core::ai_store::{ResolvedTask, TASK_CHAT_TITLE};
use rrss_core::network::NetworkScope;
use rrss_core::{Error, Result, db, threads};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use tokio::sync::OwnedMutexGuard;

use crate::scheduler::model_capacity::Capacity;
use crate::{
    network::NetworkClients,
    views::{ModelChoice, ThreadTitleOutcome},
};

use super::{complete, spawn_db};

pub(crate) const AUTO_TITLE_KEY: &str = "chat_auto_title";
pub(crate) const TITLE_MAX: usize = 80;
const QUESTION_CHARS: usize = 240;
const AUTO_REVISION_KEY: &str = "chat_title_auto_revision";
const BINDING_REVISION_KEY: &str = "chat_title_binding_revision";

pub(crate) struct TitleRequest {
    authorization: Authorization,
    questions: Vec<String>,
    _guard: OwnedMutexGuard<()>,
    _capacity: Arc<Capacity>,
}

#[derive(Clone)]
struct Authorization {
    thread_id: i64,
    token: String,
    automatic: bool,
    auto_revision: Option<String>,
    binding_revision: Option<String>,
}

impl TitleRequest {
    pub(crate) fn thread_id(&self) -> i64 {
        self.authorization.thread_id
    }
}

impl Authorization {
    fn authorization_current(&self, conn: &Connection) -> Result<bool> {
        Ok(
            self.binding_revision == db::get_setting(conn, BINDING_REVISION_KEY)?
                && (!self.automatic
                    || (db::setting_enabled(conn, AUTO_TITLE_KEY, false)?
                        && self.auto_revision == db::get_setting(conn, AUTO_REVISION_KEY)?)),
        )
    }

    fn ticket_current(&self, conn: &Connection) -> Result<bool> {
        let token: Option<String> = conn
            .query_row(
                "SELECT title_request_token FROM threads WHERE id = ?1 AND anchor_kind IS NULL",
                [self.thread_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        Ok(token.as_deref() == Some(&self.token) && self.authorization_current(conn)?)
    }

    fn apply(
        &self,
        conn: &Connection,
        resolved: &ResolvedTask,
        title: &str,
    ) -> Result<ThreadTitleOutcome> {
        db::atomic_write(conn, |conn| {
            if !self.authorization_current(conn)? || !complete::task_is_current(conn, resolved)? {
                return Ok(ThreadTitleOutcome::Stale);
            }
            let updated = conn.execute(
                "UPDATE threads SET title = ?1, title_request_token = NULL,
                     title_user_edited = CASE WHEN ?4 THEN title_user_edited ELSE 1 END
                 WHERE id = ?2 AND title_request_token = ?3 AND anchor_kind IS NULL",
                params![title, self.thread_id, self.token, self.automatic],
            )?;
            Ok(if updated == 1 {
                ThreadTitleOutcome::Applied
            } else {
                ThreadTitleOutcome::Stale
            })
        })
    }
}

pub(crate) fn save_successful_reply(
    conn: &Connection,
    db_path: &Path,
    thread_id: i64,
    body: &str,
) -> Result<(i64, Option<TitleRequest>)> {
    let tx = db::write_transaction(conn)?;
    let message =
        threads::insert_message(&tx, thread_id, threads::MessageRole::Assistant, body, None)?;
    let turns = tx
        .query_row(
            "UPDATE threads SET title_successful_turns = title_successful_turns + 1
             WHERE id = ?1 AND anchor_kind IS NULL AND title_successful_turns < 2
             RETURNING title_successful_turns",
            [thread_id],
            |row| row.get::<_, u8>(0),
        )
        .optional()?;
    let request = if turns == Some(2) && db::setting_enabled(&tx, AUTO_TITLE_KEY, false)? {
        claim(&tx, db_path, thread_id, true)?
    } else {
        None
    };
    tx.commit()?;
    Ok((message.id, request))
}

fn claim_manual(
    conn: &Connection,
    db_path: &Path,
    thread_id: i64,
) -> Result<std::result::Result<TitleRequest, ThreadTitleOutcome>> {
    let tx = db::write_transaction(conn)?;
    let thread = threads::get_thread(&tx, thread_id)?;
    if thread.anchor_kind.is_some() {
        return Ok(Err(ThreadTitleOutcome::NoQuestions));
    }
    let questions = question_fragments(&tx, thread_id)?;
    let request = claim_with_questions(&tx, db_path, thread_id, false, questions)?;
    tx.commit()?;
    Ok(request)
}

pub(crate) fn set_auto_enabled(conn: &Connection, key: &str, value: &str) -> Result<()> {
    db::atomic_write(conn, |conn| {
        if db::setting_enabled(conn, key, false)? != (value == "1") {
            conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, lower(hex(randomblob(16))))
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [AUTO_REVISION_KEY],
            )?;
        }
        db::set_setting(conn, key, value)
    })
}

pub(crate) async fn request_manual(
    db_path: &Path,
    thread_id: i64,
) -> anyhow::Result<std::result::Result<TitleRequest, ThreadTitleOutcome>> {
    let path = db_path.to_path_buf();
    spawn_db(path.clone(), move |conn| {
        claim_manual(conn, &path, thread_id)
    })
    .await
}

pub(crate) async fn generate(
    db_path: &Path,
    network: &NetworkClients,
    request: &TitleRequest,
    expected: Option<ModelChoice>,
) -> anyhow::Result<ThreadTitleOutcome> {
    let input = serde_json::to_string(&request.questions)?;
    let prepare_input = input.clone();
    let authorization = request.authorization.clone();
    let prepared = spawn_db(db_path.to_path_buf(), move |conn| {
        db::read_snapshot(conn, |conn| {
            if !authorization.ticket_current(conn)? {
                return Ok(Err(ThreadTitleOutcome::Stale));
            }
            let resolved = match crate::ai_language::resolve_task(conn, TASK_CHAT_TITLE) {
                Ok(task) => task,
                Err(Error::TaskModelUnset(_) | Error::ProviderNotFound) => {
                    return Ok(Err(ThreadTitleOutcome::NoModel));
                }
                Err(error) => return Err(error),
            };
            if expected.is_some_and(|choice| {
                choice.provider_id != resolved.provider.id || choice.model != resolved.model
            }) {
                return Ok(Err(ThreadTitleOutcome::ModelChanged));
            }
            let mut prepared = complete::prepare_resolved(
                conn,
                resolved,
                &prepare_input,
                authorization.automatic,
            )?;
            let snapshot = prepared.resolved.clone();
            prepared.dispatch_check = Some(Arc::new(move |conn| {
                Ok(authorization.ticket_current(conn)?
                    && complete::task_is_current(conn, &snapshot)?)
            }));
            Ok(Ok(prepared))
        })
    })
    .await?;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(outcome) => return Ok(outcome),
    };
    let client = network.client(NetworkScope::Api).await?;
    let resolved = prepared.resolved.clone();
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "title": {
                "type": "string",
                "description": "Concise plain-text topic title (4–16 CJK characters or 3–8 English words, max 80 chars, no quotes or trailing period)."
            }
        },
        "required": ["title"],
        "additionalProperties": false
    });
    let result = complete::execute_prepared(db_path, &client, prepared, &input, schema, |value| {
        Ok(validated_title(value).map(str::to_owned))
    })
    .await;
    let title = match result {
        Err(error)
            if error.downcast_ref::<rrss_ai::Error>().is_some_and(|error| {
                matches!(
                    error,
                    rrss_ai::Error::Admission(rrss_core::error::JobFailure::ConfigChanged)
                )
            }) || error.downcast_ref::<Error>().is_some_and(|error| {
                matches!(
                    error,
                    Error::Jobs(rrss_core::error::JobFailure::ConfigChanged)
                )
            }) =>
        {
            return Ok(ThreadTitleOutcome::Stale);
        }
        result => result?,
    };
    let Some(title) = title else {
        return Ok(ThreadTitleOutcome::Invalid);
    };
    let authorization = request.authorization.clone();
    spawn_db(db_path.to_path_buf(), move |conn| {
        authorization.apply(conn, &resolved, &title)
    })
    .await
}

pub(crate) fn store_manual_title(
    conn: &Connection,
    thread_id: i64,
    title: Option<&str>,
) -> Result<bool> {
    let updated = conn.execute(
        "UPDATE threads SET title = ?1, title_user_edited = 1, title_request_token = NULL
         WHERE id = ?2 AND anchor_kind IS NULL",
        params![title, thread_id],
    )?;
    Ok(updated == 1)
}

fn claim(
    conn: &Connection,
    db_path: &Path,
    thread_id: i64,
    automatic: bool,
) -> Result<Option<TitleRequest>> {
    let questions = question_fragments(conn, thread_id)?;
    Ok(claim_with_questions(conn, db_path, thread_id, automatic, questions)?.ok())
}

fn claim_with_questions(
    conn: &Connection,
    db_path: &Path,
    thread_id: i64,
    automatic: bool,
    questions: Vec<String>,
) -> Result<std::result::Result<TitleRequest, ThreadTitleOutcome>> {
    if questions.is_empty() {
        return Ok(Err(ThreadTitleOutcome::NoQuestions));
    }
    let capacity = Capacity::for_database(db_path);
    let lock = capacity.object(format!("chat_title:{thread_id}"));
    let Ok(guard) = lock.try_lock_owned() else {
        return Ok(Err(ThreadTitleOutcome::Busy));
    };
    let token: Option<String> = conn
        .query_row(
            "UPDATE threads SET title_request_token = lower(hex(randomblob(16)))
             WHERE id = ?1 AND anchor_kind IS NULL AND (?2 = 0 OR title_user_edited = 0)
             RETURNING title_request_token",
            params![thread_id, automatic],
            |row| row.get(0),
        )
        .optional()?;
    let Some(token) = token else {
        return Ok(Err(ThreadTitleOutcome::Stale));
    };
    Ok(Ok(TitleRequest {
        authorization: Authorization {
            thread_id,
            token,
            automatic,
            auto_revision: if automatic {
                db::get_setting(conn, AUTO_REVISION_KEY)?
            } else {
                None
            },
            binding_revision: db::get_setting(conn, BINDING_REVISION_KEY)?,
        },
        questions,
        _guard: guard,
        _capacity: capacity,
    }))
}

fn question_fragments(conn: &Connection, thread_id: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT substr(ltrim(content), 1, ?2) FROM messages
         WHERE thread_id = ?1 AND role = 'user' AND trim(content) <> ''
         ORDER BY created_at, id LIMIT 2",
    )?;
    let rows = stmt.query_map(params![thread_id, QUESTION_CHARS], |row| {
        row.get::<_, String>(0)
    })?;
    let mut questions = Vec::new();
    for row in rows {
        let text = row?;
        let text = text.trim();
        if !text.is_empty() {
            questions.push(text.to_owned());
        }
    }
    Ok(questions)
}

fn validated_title(response: &Value) -> Option<&str> {
    let raw = response.get("title")?.as_str()?;
    let title = raw.trim();
    if title.is_empty()
        || title.chars().count() > TITLE_MAX
        || raw.contains(|ch: char| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
    {
        return None;
    }
    Some(title)
}

#[cfg(test)]
mod tests;
