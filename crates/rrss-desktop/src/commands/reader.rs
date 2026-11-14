use tauri::{AppHandle, Emitter, State};

use rrss_app::queries;
use rrss_app::views::*;

use crate::state::{AppState, with_db};

db_task!(mark_read(id: i64, read: bool) -> () =>
    |conn| rrss_app::actions::mark_read(conn, id, read));

db_task!(star_article(id: i64, starred: bool) -> () =>
    |conn| rrss_app::actions::star_article(conn, id, starred));

db_task!(unmark_read(ids: Vec<i64>) -> () => |conn| rrss_app::actions::unmark_read(conn, &ids));

#[tauri::command]
pub async fn fetch_fulltext(state: State<'_, AppState>, id: i64) -> Result<ReaderView, UiError> {
    let path = state.db_path.clone();
    rrss_app::fulltext::fetch_and_store(&state.network, &path, id)
        .await
        .map_err(UiError::from)?;
    with_db("fetch_fulltext", path, move |conn| {
        queries::reader_view(conn, id).map_err(UiError::from)
    })
    .await
}

db_task!(article_reader(id: i64) -> ReaderView => |conn| queries::reader_view(conn, id));

db_task!(related_articles(article_id: i64, limit: i64) -> Vec<RelatedArticleView> =>
    |conn| queries::related_articles(conn, article_id, limit));

db_task!(open_article(id: i64) -> ReaderView =>
    |conn| rrss_app::actions::open_article(conn, id));

db_task!(rate_article(id: i64, up: bool) -> () =>
    |conn| rrss_app::actions::rate_article(conn, id, up).map(|_| ()));

db_task!(cite_article(id: i64) -> CitationView =>
    |conn| rrss_app::actions::cite_article(conn, id));

db_task!(cite_selection(article_id: i64, excerpt: String, block_id: i64) -> CitationView =>
    |conn| rrss_app::actions::cite_selection(conn, article_id, &excerpt, block_id));

#[tauri::command]
pub async fn cite_digest_selection(
    state: State<'_, AppState>,
    digest_id: i64,
    revision: String,
    selections: Vec<DigestSelection>,
) -> Result<Vec<CitationView>, UiError> {
    with_db(
        "cite_digest_selection",
        state.db_path.clone(),
        move |conn| {
            rrss_app::digest::cite_selection(conn, digest_id, &revision, &selections)
                .map_err(UiError::from_anyhow_pretty)
        },
    )
    .await
}

#[tauri::command]
pub async fn prefetch_images(
    app: AppHandle,
    state: State<'_, AppState>,
    article_id: i64,
) -> Result<Vec<MediaRef>, UiError> {
    rrss_app::jobs::media::prefetch_images(
        &state.db_path,
        &state.data_dir,
        &state.network,
        article_id,
        move |event| {
            let _ = app.emit("media:ready", event);
        },
    )
    .await
    .map_err(UiError::from)
}

#[tauri::command]
pub async fn retry_image(
    state: State<'_, AppState>,
    article_id: i64,
    url: String,
) -> Result<MediaRef, UiError> {
    rrss_app::jobs::media::retry_image(
        &state.db_path,
        &state.data_dir,
        &state.network,
        article_id,
        &url,
    )
    .await
    .map_err(UiError::from)
}

#[tauri::command]
pub async fn prefetch_embed_covers(
    app: AppHandle,
    state: State<'_, AppState>,
    article_id: i64,
) -> Result<(), UiError> {
    rrss_app::jobs::media::prefetch_embed_covers(
        &state.db_path,
        &state.data_dir,
        &state.network,
        article_id,
        move |event| {
            let _ = app.emit("embed:cover", event);
        },
    )
    .await
    .map_err(UiError::from)
}

#[tauri::command]
pub async fn download_direct_media(
    state: State<'_, AppState>,
    article_id: i64,
    url: String,
) -> Result<MediaRef, UiError> {
    let referer = with_db(
        "download_direct_media",
        state.db_path.clone(),
        move |conn| {
            Ok(rrss_core::articles::get_article(conn, article_id)
                .map(|article| article.canonical_url.unwrap_or_default())
                .unwrap_or_default())
        },
    )
    .await?;
    rrss_app::embed_fetch::download_direct(
        &state.network,
        &state.db_path,
        &state.data_dir,
        article_id,
        &url,
        &referer,
    )
    .await
    .map_err(UiError::from)
}

#[tauri::command]
pub async fn open_web_link(
    state: State<'_, AppState>,
    url: String,
) -> Result<OpenWebLinkView, rrss_app::fulltext::FulltextError> {
    rrss_app::actions::open_web_link(&state.network, &state.db_path, &url).await
}

db_task!(collect_article(id: i64) -> () => |conn| rrss_app::actions::collect_article(conn, id));

#[tauri::command]
pub async fn link_safety(
    href: String,
    link_text: String,
) -> Result<LinkSafety, rrss_app::fulltext::FulltextError> {
    rrss_app::execution::blocking(move || rrss_app::web::link_safety(&href, &link_text))
        .await
        .map_err(rrss_app::fulltext::FulltextError::network)?
}

db_task!(site_rules_page() -> SiteRulesPage => rrss_app::site_rules::site_rules_page);
db_task!(save_site_rule(rule: SiteRuleView, enabled: bool) -> () => |conn| rrss_app::site_rules::save_site_rule(conn, rule, enabled));
db_task!(delete_site_rule(domain: String) -> () => |conn| rrss_app::site_rules::delete_site_rule(conn, &domain));
db_task!(set_site_rule_enabled(domain: String, enabled: bool) -> () => |conn| rrss_app::site_rules::set_site_rule_enabled(conn, &domain, enabled));

#[tauri::command]
pub async fn test_site_rule(
    state: State<'_, AppState>,
    url: String,
    rule: SiteRuleView,
) -> Result<SiteRuleTestView, rrss_app::fulltext::FulltextError> {
    rrss_app::site_rules::test_site_rule(&state.network, &url, rule).await
}

command_handler![
    article_reader,
    related_articles,
    open_article,
    mark_read,
    star_article,
    unmark_read,
    fetch_fulltext,
    open_web_link,
    collect_article,
    link_safety,
    site_rules_page,
    save_site_rule,
    delete_site_rule,
    set_site_rule_enabled,
    test_site_rule,
    rate_article,
    cite_article,
    cite_selection,
    cite_digest_selection,
    prefetch_images,
    prefetch_embed_covers,
    download_direct_media,
    retry_image,
];
