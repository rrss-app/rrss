use rrss_app::queries;
use rrss_app::views::{SearchHitBlocksView, SearchHitView, SearchQuery};

db_task!(search_articles(query: SearchQuery) -> Vec<SearchHitView> =>
    |conn| queries::search_hits(conn, &query));

db_task!(search_hit_blocks(article_id: i64, query: String) -> SearchHitBlocksView =>
    |conn| queries::search_hit_blocks(conn, article_id, &query));

command_handler![search_articles, search_hit_blocks,];
