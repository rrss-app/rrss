//! 阶段 23「接入方式」：原始信号命令转接。

use rrss_app::views::{ArticleEventInput, ExposureInput, ReadSessionInput, ReadingSpeedsView};

db_task!(record_exposures(records: Vec<ExposureInput>) -> Vec<Option<i64>> =>
    |conn| rrss_app::actions::record_exposures(conn, records));

db_task!(record_article_events(records: Vec<ArticleEventInput>) -> usize =>
    |conn| rrss_app::actions::record_article_events(conn, records));

db_task!(record_read_sessions(records: Vec<ReadSessionInput>) -> usize =>
    |conn| rrss_app::actions::record_read_sessions(conn, records));

db_task!(reading_speeds() -> ReadingSpeedsView =>
    rrss_app::queries::reader::reading_speeds);

command_handler![
    record_exposures,
    record_article_events,
    record_read_sessions,
    reading_speeds,
];
