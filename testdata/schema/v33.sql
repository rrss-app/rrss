-- 来源：origin/main:crates/rrss-core/src/schema.sql @ 3784c257（SCHEMA_VERSION 33）
-- schema v1：阶段 0 一次建齐（含阶段 2–4 空表）。时间一律 Unix 毫秒。

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT INTO settings (key, value) VALUES ('theme_preference', 'system');
INSERT INTO settings (key, value) VALUES ('ai_daily_budget_tokens', '0');
INSERT INTO settings (key, value) VALUES ('developer_tools', '0');
INSERT INTO settings (key, value) VALUES ('fetch_url_enabled', '0');
INSERT INTO settings (key, value) VALUES ('retention_days', '0');
INSERT INTO settings (key, value) VALUES ('interest_boost_enabled', '1');
INSERT INTO settings (key, value) VALUES ('focus_rerank_enabled', '0');
INSERT INTO settings (key, value) VALUES ('cluster_group_enabled', '1');
INSERT INTO settings (key, value) VALUES ('pi_idle_timeout_secs', '1800');
INSERT INTO settings (key, value) VALUES ('focus_summary_neutral_limit', '0');
INSERT INTO settings (key, value) VALUES ('pi_auto_download', '1');
INSERT INTO settings (key, value) VALUES ('feed_icons_enabled', '1');
INSERT INTO settings (key, value) VALUES ('pi_auto_update', '0');
INSERT INTO settings (key, value) VALUES ('pi_managed_version', '');

CREATE TABLE folders (
    id         INTEGER PRIMARY KEY,
    parent_id  INTEGER REFERENCES folders (id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    position   INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE TABLE rsshub_instances (
    id         INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL,
    base_url   TEXT    NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);

CREATE TABLE feeds (
    id                  INTEGER PRIMARY KEY,
    url                 TEXT    NOT NULL UNIQUE,
    title               TEXT,
    kind                TEXT    NOT NULL DEFAULT 'rss'
                        CHECK (kind IN ('rss', 'atom', 'json', 'rsshub', 'manual')),
    site_url            TEXT,
    icon_checked_at     INTEGER,
    etag                TEXT,
    last_modified       TEXT,
    interval_secs       INTEGER NOT NULL DEFAULT 3600,
    folder_id           INTEGER REFERENCES folders (id) ON DELETE SET NULL,
    error               TEXT,
    rsshub_instance_id  INTEGER REFERENCES rsshub_instances (id) ON DELETE SET NULL,
    last_fetched_at     INTEGER,
    created_at          INTEGER NOT NULL,
    -- 1：名称由用户或导入的 OPML 指定，刷新不再用源标题覆盖
    title_custom        INTEGER NOT NULL DEFAULT 0,
    -- 保留天数：NULL 跟随全局设置，0 永久保留
    retention_days      INTEGER,
    -- 源健康：连续失败次数、这轮失败开始时间、最近成功（200 / 304）
    fail_count          INTEGER NOT NULL DEFAULT 0,
    failing_since       INTEGER,
    last_success_at     INTEGER,
    -- 用户忽略健康提示的时间与当时的状态种类；种类变化或满 30 天后再提示
    health_dismissed_at   INTEGER,
    health_dismissed_kind TEXT
);

CREATE INDEX idx_feeds_folder ON feeds (folder_id);
CREATE UNIQUE INDEX idx_feeds_manual ON feeds(kind) WHERE kind='manual';

CREATE TABLE articles (
    id              INTEGER PRIMARY KEY,
    feed_id         INTEGER NOT NULL REFERENCES feeds (id) ON DELETE CASCADE,
    guid            TEXT,
    canonical_url   TEXT,
    title           TEXT    NOT NULL DEFAULT '',
    author          TEXT,
    published_at    INTEGER,
    content_html    TEXT,
    content_md      TEXT,
    fulltext_md     TEXT,
    blocks_json     TEXT,
    hash            TEXT,
    simhash         INTEGER,
    read            INTEGER NOT NULL DEFAULT 0,
    starred         INTEGER NOT NULL DEFAULT 0,
    score           INTEGER NOT NULL DEFAULT 0,
    interest_boost  INTEGER NOT NULL DEFAULT 0,
    cluster_id      INTEGER,
    summary         TEXT,
    summary_json    TEXT,
    summary_hash    TEXT,
    reasons_json    TEXT,
    released_at     INTEGER,
    interest_reasons_json TEXT,
    created_at      INTEGER NOT NULL,
    -- 按保留策略清掉正文的时间；NULL 表示正文完整
    pruned_at       INTEGER,
    expires_at      INTEGER,
    web_extracted_at INTEGER
);

CREATE TABLE article_rank (
    article_id INTEGER PRIMARY KEY REFERENCES articles(id) ON DELETE CASCADE,
    boost INTEGER NOT NULL,
    reasons_json TEXT NOT NULL,
    algo_version INTEGER NOT NULL,
    computed_at INTEGER NOT NULL
);

CREATE INDEX idx_articles_expires ON articles(expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX idx_articles_feed ON articles (feed_id);
CREATE INDEX idx_articles_published ON articles (published_at DESC);
CREATE INDEX idx_articles_unread ON articles (read, published_at DESC);
CREATE INDEX idx_articles_canonical ON articles (canonical_url);
CREATE INDEX idx_articles_guid ON articles (feed_id, guid);
CREATE UNIQUE INDEX idx_articles_guid_uid ON articles (feed_id, guid) WHERE guid IS NOT NULL;
CREATE INDEX idx_articles_hash ON articles (hash);
CREATE INDEX idx_articles_cluster ON articles (cluster_id);
CREATE INDEX idx_articles_simhash ON articles (simhash);
CREATE INDEX idx_articles_unpruned ON articles (created_at) WHERE pruned_at IS NULL;

CREATE TABLE clusters (
    id                   INTEGER PRIMARY KEY,
    label                TEXT    NOT NULL DEFAULT '',
    representative_id    INTEGER,
    member_count         INTEGER NOT NULL DEFAULT 1,
    first_published_at   INTEGER,
    last_published_at    INTEGER,
    created_at           INTEGER NOT NULL
);

CREATE VIRTUAL TABLE articles_fts USING fts5 (
    title,
    author,
    body,
    tokenize = 'trigram'
);

CREATE TRIGGER articles_ai AFTER INSERT ON articles BEGIN
    INSERT INTO articles_fts (rowid, title, author, body)
    VALUES (
        new.id,
        new.title,
        coalesce(new.author, ''),
        coalesce(new.fulltext_md, new.content_md, new.content_html, '')
    );
END;

CREATE TRIGGER articles_ad AFTER DELETE ON articles BEGIN
    DELETE FROM articles_fts WHERE rowid = old.id;
END;

CREATE TRIGGER articles_au AFTER UPDATE OF title, author, content_html, content_md, fulltext_md ON articles BEGIN
    DELETE FROM articles_fts WHERE rowid = old.id;
    INSERT INTO articles_fts (rowid, title, author, body)
    VALUES (
        new.id,
        new.title,
        coalesce(new.author, ''),
        coalesce(new.fulltext_md, new.content_md, new.content_html, '')
    );
END;

CREATE TABLE media (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    article_id  INTEGER REFERENCES articles (id) ON DELETE CASCADE,
    feed_id     INTEGER REFERENCES feeds (id) ON DELETE CASCADE,
    url         TEXT    NOT NULL,
    local_path  TEXT,
    mime        TEXT,
    kind        TEXT    NOT NULL DEFAULT 'image'
                CHECK (kind IN ('image', 'video', 'iframe', 'other')),
    bytes       INTEGER,
    created_at  INTEGER NOT NULL,
    accessed_at INTEGER NOT NULL DEFAULT 0
);

CREATE UNIQUE INDEX idx_media_feed_icon ON media (feed_id);
CREATE INDEX idx_media_article ON media (article_id);
CREATE INDEX idx_media_accessed ON media (accessed_at);

CREATE TABLE article_tags (
    article_id INTEGER NOT NULL REFERENCES articles (id) ON DELETE CASCADE,
    tag        TEXT    NOT NULL,
    PRIMARY KEY (article_id, tag)
);

CREATE TABLE rules (
    id          INTEGER PRIMARY KEY,
    polarity    TEXT    NOT NULL CHECK (polarity IN ('like', 'dislike')),
    dimension   TEXT    NOT NULL
                CHECK (dimension IN (
                    'author', 'tag',
                    'title_kw', 'title_re',
                    'url_phrase', 'url_re',
                    'body_phrase', 'body_re'
                )),
    pattern     TEXT    NOT NULL,
    scope_kind  TEXT    NOT NULL DEFAULT 'global'
                CHECK (scope_kind IN ('global', 'folder', 'feed')),
    scope_id    INTEGER,
    weight      INTEGER NOT NULL DEFAULT 10,
    source      TEXT    NOT NULL DEFAULT 'manual'
                CHECK (source IN ('manual', 'suggested')),
    enabled     INTEGER NOT NULL DEFAULT 1,
    pack_id     TEXT,
    created_at  INTEGER NOT NULL
);

CREATE INDEX idx_rules_scope ON rules (scope_kind, scope_id);
CREATE INDEX idx_rules_enabled ON rules (enabled);
CREATE INDEX idx_rules_pack ON rules (pack_id);

CREATE TABLE feedback (
    id          INTEGER PRIMARY KEY,
    article_id  INTEGER NOT NULL REFERENCES articles (id) ON DELETE CASCADE,
    kind        TEXT    NOT NULL CHECK (kind IN ('up', 'down')),
    created_at  INTEGER NOT NULL
);

CREATE INDEX idx_feedback_article ON feedback (article_id);
CREATE UNIQUE INDEX idx_feedback_article_uid ON feedback (article_id);

CREATE TABLE vector_spaces (
    id INTEGER PRIMARY KEY,
    provider_id INTEGER,
    base_url TEXT NOT NULL,
    model TEXT NOT NULL,
    file_sha256 TEXT,
    dims INTEGER NOT NULL,
    prefix_version INTEGER NOT NULL,
    normalized INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('building', 'active', 'stale', 'retired')),
    created_at INTEGER NOT NULL,
    stale_reason TEXT,
    checked_at INTEGER,
    rebuild_from INTEGER REFERENCES vector_spaces(id)
);
CREATE UNIQUE INDEX idx_vector_spaces_active ON vector_spaces(state) WHERE state = 'active';
CREATE TABLE vector_space_probes (
    space_id INTEGER NOT NULL REFERENCES vector_spaces(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    embedding BLOB NOT NULL,
    PRIMARY KEY (space_id, ordinal)
);

CREATE TABLE vector_staging (
    space_id INTEGER NOT NULL REFERENCES vector_spaces(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('chunk', 'interest', 'memory')),
    row_id INTEGER NOT NULL,
    embedding BLOB NOT NULL,
    input_text TEXT NOT NULL,
    PRIMARY KEY (space_id, kind, row_id)
);

CREATE TABLE interests (
    id          INTEGER PRIMARY KEY,
    label       TEXT    NOT NULL,
    embedding   BLOB,
    space_id INTEGER REFERENCES vector_spaces(id),
    weight      INTEGER NOT NULL DEFAULT 2,
    source      TEXT    NOT NULL DEFAULT 'manual'
                CHECK (source IN ('manual', 'from_chat')),
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL
);

-- 连接只有地址、密钥、协议与兼容设置；`model` 自 v20 起不再读写（阶段 19-7），列保留。
-- `api_key` 为空串表示从不含密钥的备份恢复、需要重新填写；NULL 表示没有密钥。
CREATE TABLE ai_providers (
    id             INTEGER PRIMARY KEY,
    name           TEXT    NOT NULL,
    base_url       TEXT    NOT NULL,
    api_key        TEXT,
    model          TEXT,
    kind           TEXT    NOT NULL DEFAULT 'openai_compat',
    protocol       TEXT    NOT NULL DEFAULT 'chat',
    created_at     INTEGER NOT NULL,
    compat_json    TEXT    NOT NULL DEFAULT '{}',
    -- 最近一次「测试」：时间、是否通过、失败原因或返回的模型数。
    last_test_at   INTEGER,
    last_test_ok   INTEGER,
    last_test_note TEXT
);

-- 连接上获取到的模型 id；重新获取整体覆盖。
CREATE TABLE ai_model_catalog (
    provider_id INTEGER NOT NULL REFERENCES ai_providers (id) ON DELETE CASCADE,
    model       TEXT    NOT NULL,
    fetched_at  INTEGER NOT NULL,
    PRIMARY KEY (provider_id, model)
);

-- 用户加进来的模型（阶段 19-7）：所有选模型的地方只列这里。`chat` 为 1 的出现在对话列表。
-- 能力 JSON 的键：tool_calling / structured_output / image_input / reasoning，
-- 值 supported / unsupported（`probe_json` 另有 unknown、`tested_at` 与推荐兼容值）。
CREATE TABLE ai_model_pool (
    provider_id        INTEGER NOT NULL REFERENCES ai_providers (id) ON DELETE CASCADE,
    model              TEXT    NOT NULL,
    created_at         INTEGER NOT NULL,
    chat               INTEGER NOT NULL DEFAULT 0,
    display_name       TEXT    NOT NULL DEFAULT '',
    -- 对话里的思考强度；空为关。
    thinking           TEXT    NOT NULL DEFAULT '',
    context_window     INTEGER,
    max_output         INTEGER,
    -- NULL 用连接的兼容设置。
    compat_json        TEXT,
    probe_json         TEXT,
    caps_override_json TEXT    NOT NULL DEFAULT '{}',
    PRIMARY KEY (provider_id, model)
);

CREATE TABLE prompt_templates (
    id      INTEGER PRIMARY KEY,
    slug    TEXT    NOT NULL UNIQUE,
    title   TEXT    NOT NULL,
    body    TEXT    NOT NULL,
    version INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE task_bindings (
    task         TEXT    PRIMARY KEY,
    provider_id  INTEGER REFERENCES ai_providers (id) ON DELETE SET NULL,
    template_id  INTEGER REFERENCES prompt_templates (id) ON DELETE SET NULL,
    model        TEXT
);

CREATE TABLE ai_cache (
    cache_key   TEXT    PRIMARY KEY,
    value       TEXT    NOT NULL,
    created_at  INTEGER NOT NULL
);

CREATE TABLE ai_usage (
    day               TEXT    PRIMARY KEY,
    tokens            INTEGER NOT NULL DEFAULT 0,
    prompt_tokens     INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE threads (
    id               INTEGER PRIMARY KEY,
    title            TEXT,
    pinned           INTEGER NOT NULL DEFAULT 0,
    anchor_kind      TEXT,
    anchor_id        INTEGER,
    pi_session_file  TEXT,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    -- 这条对话选定的模型；为空跟随对话默认。provider 为 0 表示自有 Pi 配置下的 `provider/id`。
    chat_provider_id INTEGER,
    chat_model       TEXT,
    chat_thinking    TEXT,
    context_used     INTEGER,
    context_window   INTEGER,
    context_at       INTEGER
);

CREATE TABLE messages (
    id              INTEGER PRIMARY KEY,
    thread_id       INTEGER NOT NULL REFERENCES threads (id) ON DELETE CASCADE,
    role            TEXT    NOT NULL CHECK (role IN ('user', 'assistant', 'system')),
    content         TEXT    NOT NULL DEFAULT '',
    citations_json  TEXT,
    created_at      INTEGER NOT NULL
);

CREATE INDEX idx_messages_thread ON messages (thread_id, created_at);

CREATE TABLE memories (
    id            INTEGER PRIMARY KEY,
    kind          TEXT    NOT NULL
                  CHECK (kind IN ('episode', 'semantic', 'procedural')),
    content       TEXT    NOT NULL,
    embedding     BLOB,
    space_id INTEGER REFERENCES vector_spaces(id),
    scope         TEXT    NOT NULL DEFAULT 'global'
                  CHECK (scope IN ('global', 'folder', 'feed', 'thread')),
    scope_id      INTEGER,
    pinned        INTEGER NOT NULL DEFAULT 0,
    strength      REAL    NOT NULL DEFAULT 1.0,
    source        TEXT,
    last_used_at  INTEGER,
    created_at    INTEGER NOT NULL
);

CREATE TABLE chunks (
    id          INTEGER PRIMARY KEY,
    article_id  INTEGER NOT NULL REFERENCES articles (id) ON DELETE CASCADE,
    ordinal     INTEGER NOT NULL,
    kind        TEXT    NOT NULL DEFAULT 'body'
                CHECK (kind IN ('title_summary', 'body')),
    text        TEXT    NOT NULL,
    embedding   BLOB,
    space_id INTEGER REFERENCES vector_spaces(id),
    model       TEXT,
    source_hash TEXT,
    attempts    INTEGER NOT NULL DEFAULT 0,
    last_error  TEXT,
    UNIQUE (article_id, ordinal)
);

CREATE INDEX idx_chunks_space ON chunks(space_id);
CREATE INDEX idx_interests_space ON interests(space_id);
CREATE INDEX idx_memories_space ON memories(space_id);

CREATE INDEX idx_chunks_article ON chunks (article_id);

CREATE TABLE article_interests (
    article_id  INTEGER NOT NULL REFERENCES articles (id) ON DELETE CASCADE,
    interest_id INTEGER NOT NULL REFERENCES interests (id) ON DELETE CASCADE,
    weight      INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (article_id, interest_id)
);

CREATE INDEX idx_article_interests_interest ON article_interests (interest_id);

CREATE TABLE pi_tools (
    name                 TEXT    PRIMARY KEY,
    enabled              INTEGER NOT NULL DEFAULT 1,
    label                TEXT    NOT NULL,
    description          TEXT    NOT NULL DEFAULT '',
    confirm_required     INTEGER NOT NULL DEFAULT 0,
    builtin              INTEGER NOT NULL DEFAULT 1,
    default_description  TEXT    NOT NULL DEFAULT ''
);

CREATE TABLE mcp_servers (
    id                  INTEGER PRIMARY KEY,
    name                TEXT    NOT NULL UNIQUE,
    transport           TEXT    NOT NULL DEFAULT 'stdio',
    command             TEXT,
    args_json           TEXT,
    url                 TEXT,
    env_json            TEXT,
    headers_json        TEXT,
    lifecycle           TEXT    NOT NULL DEFAULT 'lazy',
    include_tools_json  TEXT,
    enabled             INTEGER NOT NULL DEFAULT 0,
    created_at          INTEGER NOT NULL
);

CREATE TABLE pi_usage (
    day            TEXT    PRIMARY KEY,
    input_tokens   INTEGER NOT NULL DEFAULT 0,
    output_tokens  INTEGER NOT NULL DEFAULT 0,
    cost           REAL    NOT NULL DEFAULT 0
);

CREATE TABLE digests (
    id            INTEGER PRIMARY KEY,
    period        TEXT    NOT NULL CHECK (period IN ('daily', 'weekly')),
    period_start  INTEGER NOT NULL,
    body          TEXT    NOT NULL,
    items_json    TEXT    NOT NULL DEFAULT '[]',
    created_at    INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_digests_period_start ON digests (period, period_start);

CREATE TABLE article_translations (
    article_id     INTEGER NOT NULL REFERENCES articles (id) ON DELETE CASCADE,
    target_lang    TEXT    NOT NULL,
    source_hash    TEXT    NOT NULL,
    engine         TEXT    NOT NULL DEFAULT 'complete',
    model          TEXT,
    prompt_version INTEGER NOT NULL DEFAULT 1,
    title          TEXT    NOT NULL DEFAULT '',
    blocks_json    TEXT    NOT NULL,
    created_at     INTEGER NOT NULL,
    PRIMARY KEY (article_id, target_lang, engine)
);

CREATE TABLE exposures (
    id INTEGER PRIMARY KEY,
    article_id INTEGER NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    surface TEXT NOT NULL,
    position INTEGER NOT NULL,
    score INTEGER NOT NULL,
    interest_boost INTEGER NOT NULL,
    rank_score REAL NOT NULL,
    variant TEXT NOT NULL,
    session_id TEXT NOT NULL,
    shown_at INTEGER NOT NULL,
    UNIQUE(session_id, article_id)
);
CREATE INDEX idx_exposures_time ON exposures(shown_at);
CREATE INDEX idx_exposures_article ON exposures(article_id);

CREATE TABLE article_events (
    id INTEGER PRIMARY KEY,
    article_id INTEGER NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    exposure_id INTEGER REFERENCES exposures(id) ON DELETE SET NULL,
    at INTEGER NOT NULL
);
CREATE INDEX idx_article_events_time ON article_events(at);
CREATE INDEX idx_article_events_article ON article_events(article_id);
CREATE INDEX idx_article_events_exposure ON article_events(exposure_id);

CREATE TABLE read_sessions (
    id INTEGER PRIMARY KEY,
    article_id INTEGER NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    exposure_id INTEGER REFERENCES exposures(id) ON DELETE SET NULL,
    opened_at INTEGER NOT NULL,
    active_ms INTEGER NOT NULL,
    idle_ms INTEGER NOT NULL,
    read_chars INTEGER NOT NULL,
    skimmed_chars INTEGER NOT NULL,
    total_chars INTEGER NOT NULL,
    max_block INTEGER NOT NULL,
    reached_end INTEGER NOT NULL,
    translated INTEGER NOT NULL,
    ended_by TEXT NOT NULL,
    via TEXT NOT NULL
);
CREATE INDEX idx_read_sessions_time ON read_sessions(opened_at);
CREATE INDEX idx_read_sessions_article ON read_sessions(article_id);
CREATE INDEX idx_read_sessions_exposure ON read_sessions(exposure_id);

-- Declarative web extraction rules; strip is a JSON selector array or NULL.
CREATE TABLE site_rules (
  javascript_disabled INTEGER NOT NULL DEFAULT 0,
    id INTEGER PRIMARY KEY,
    domain TEXT NOT NULL UNIQUE,
    body TEXT,
    title TEXT,
    author TEXT,
    date TEXT,
    strip TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    updated_at INTEGER NOT NULL
);
