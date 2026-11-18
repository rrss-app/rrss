//! Reading activity write orchestration: exposures, article events and read sessions.

use rusqlite::Connection;

use crate::views::{ArticleEventInput, ExposureIds, ExposureInput, ReadSessionInput};

pub fn record_exposures(
    conn: &Connection,
    records: Vec<ExposureInput>,
) -> rrss_core::Result<ExposureIds> {
    rrss_core::signals::record_exposures(
        conn,
        &records.into_iter().map(Into::into).collect::<Vec<_>>(),
    )
}

pub fn record_article_events(
    conn: &Connection,
    records: Vec<ArticleEventInput>,
) -> rrss_core::Result<usize> {
    let count = rrss_core::signals::record_events(
        conn,
        &records.into_iter().map(Into::into).collect::<Vec<_>>(),
    )?;
    if count > 0 {
        crate::jobs::rank::request(conn)?;
    }
    Ok(count)
}

pub fn record_read_sessions(
    conn: &Connection,
    records: Vec<ReadSessionInput>,
) -> rrss_core::Result<usize> {
    let count = rrss_core::signals::record_read_sessions(
        conn,
        &records.into_iter().map(Into::into).collect::<Vec<_>>(),
    )?;
    if count > 0 {
        crate::jobs::rank::request(conn)?;
    }
    Ok(count)
}
