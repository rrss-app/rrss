use super::*;
use corpus::{Corpus, InputCase};
use rrss_core::rag::{self, DigestCandidate};

pub const NOW: i64 = 1_800_000_000_000;
pub const DAY: i64 = 86_400_000;
pub const DAILY_CANDIDATES: i64 = 40;
pub const WEEKLY_CANDIDATES: i64 = 60;
pub const SUPPLEMENT_CANDIDATES: i64 = 30;
pub const SUPPLEMENT_MAX: usize = 5;
pub const PICK_MAX: usize = 12;

const FEEDS: &[(&str, &str)] = &[
    ("cedar", "Cedar Signal"),
    ("willow", "Willow Wire"),
    ("fir", "Fir Notes"),
    ("alder", "Alder Dispatch"),
    ("maple", "Maple Research"),
    ("juniper", "Juniper Commons"),
];

#[derive(Clone, Deserialize)]
pub struct Article {
    pub key: String,
    pub feed: String,
    pub title: String,
    pub body: String,
    #[serde(default = "markdown")]
    pub format: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default = "score")]
    pub score: i64,
    #[serde(default)]
    pub cluster: Option<i64>,
    #[serde(default)]
    pub age_hours: i64,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub down: bool,
    #[serde(default)]
    pub link_stub: bool,
}
fn markdown() -> String {
    "markdown".into()
}
fn score() -> i64 {
    5
}

pub struct FixtureDb {
    // Close SQLite before deleting its directory, including on Windows.
    pub conn: db::PooledConnection,
    _dir: tempfile::TempDir,
    pub ids: BTreeMap<String, i64>,
    pub link_stubs: BTreeSet<i64>,
    feeds: HashMap<String, i64>,
}

impl FixtureDb {
    pub fn new() -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let conn = db::open_at(&dir.path().join("eval.db"))?;
        for key in [
            ai_store::FOCUS_AUTO_SUMMARY.id,
            ai_store::CLUSTER_AUTO_SUMMARY.id,
        ] {
            db::set_setting(&conn, key, "0")?;
        }
        // This executable starts no scheduler, ingest, media download or embedding jobs.
        let mut feeds = HashMap::new();
        for (key, title) in FEEDS {
            conn.execute(
                "INSERT INTO feeds(url,title,created_at) VALUES(?1,?2,?3)",
                rusqlite::params![format!("https://{key}.example/feed"), title, NOW],
            )?;
            feeds.insert(key.to_string(), conn.last_insert_rowid());
        }
        Ok(Self {
            conn,
            _dir: dir,
            ids: BTreeMap::new(),
            link_stubs: BTreeSet::new(),
            feeds,
        })
    }

    pub fn insert(&mut self, article: &Article) -> Result<i64> {
        let doc = match article.format.as_str() {
            "html" => {
                rrss_render::html_to_document(&article.body, &rrss_render::RenderOpts::default())
            }
            "markdown" => rrss_render::markdown_to_document(&article.body),
            format => bail!("unsupported fixture format {format}"),
        };
        let blocks = rrss_render::document_to_json(&doc)?;
        self.insert_blocks(
            article,
            &blocks,
            (article.format == "html").then_some(article.body.as_str()),
        )
    }

    fn insert_blocks(
        &mut self,
        article: &Article,
        blocks: &str,
        html: Option<&str>,
    ) -> Result<i64> {
        let feed = self
            .feeds
            .get(&article.feed)
            .context("unknown fictional fixture feed")?;
        if let Some(cluster) = article.cluster {
            self.conn.execute("INSERT OR IGNORE INTO clusters(id,label,member_count,created_at) VALUES(?1,?2,0,?3)",
                rusqlite::params![cluster, format!("Fixture event {cluster}"), NOW])?;
        }
        let summary_json = article.summary.as_ref().map(|summary| {
            json!({
                "summary": summary, "one_liner": summary, "key_points": [], "facts": [],
            })
            .to_string()
        });
        let hash = format!("fixture-{}", article.key);
        let at = NOW - article.age_hours * 3_600_000;
        self.conn.execute(
            "INSERT INTO articles(feed_id,title,hash,created_at,published_at,score,cluster_id,
                content_html,blocks_json,canonical_url,summary,summary_json,summary_hash,read)
             VALUES(?1,?2,?3,?4,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            rusqlite::params![
                feed,
                article.title,
                hash,
                at,
                article.score,
                article.cluster,
                html,
                blocks,
                format!("https://{}.example/{}", article.feed, article.key),
                article.summary,
                summary_json,
                article.summary.as_ref().map(|_| hash.as_str()),
                i64::from(article.read)
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        if article.down {
            self.conn.execute(
                "INSERT INTO feedback(article_id,kind,created_at) VALUES(?1,'down',?2)",
                [id, NOW],
            )?;
        }
        if article.link_stub {
            self.link_stubs.insert(id);
        }
        anyhow::ensure!(
            self.ids.insert(article.key.clone(), id).is_none(),
            "duplicate fixture key"
        );
        Ok(id)
    }

    pub fn from_corpus(
        &mut self,
        input: &InputCase,
        key: &str,
        feed: &str,
        score: i64,
        cluster: Option<i64>,
        summarized: bool,
    ) -> Result<i64> {
        let prepared = input.prepare()?;
        let article = Article {
            key: key.into(),
            feed: feed.into(),
            title: input.title.clone(),
            body: String::new(),
            format: input.format.clone(),
            summary: summarized
                .then(|| format!("{}: controlled field trial completed.", input.title)),
            score,
            cluster,
            age_hours: 8,
            read: false,
            down: false,
            link_stub: input.link_stub,
        };
        self.insert_blocks(&article, &prepared.blocks, prepared.html.as_deref())
    }

    pub fn refresh_clusters(&self) -> Result<()> {
        self.conn.execute("UPDATE clusters SET member_count=(SELECT COUNT(*) FROM articles a WHERE a.cluster_id=clusters.id),
            representative_id=(SELECT MIN(id) FROM articles a WHERE a.cluster_id=clusters.id)", [])?;
        Ok(())
    }

    pub fn id(&self, key: &str) -> Result<i64> {
        self.ids
            .get(key)
            .copied()
            .with_context(|| format!("unknown fixture article {key}"))
    }
}

pub fn simple(key: &str, feed: &str, body: String, value: i64, summarized: bool) -> Article {
    Article {
        key: key.into(),
        feed: feed.into(),
        title: format!("{} verification update", key),
        summary: summarized.then(|| body.clone()),
        body,
        format: "markdown".into(),
        score: value,
        cluster: None,
        age_hours: 6,
        read: false,
        down: false,
        link_stub: false,
    }
}

pub fn scene(corpus: &Corpus, name: &str) -> Result<FixtureDb> {
    let mut fixture = FixtureDb::new()?;
    let cold = name == "cold";
    for (i, key) in ["zh-depth", "en-depth"].iter().enumerate() {
        let input = corpus
            .articles
            .iter()
            .find(|a| a.id == *key)
            .context("missing long corpus case")?;
        fixture.from_corpus(
            input,
            key,
            FEEDS[i].0,
            30 - i as i64,
            Some(7000),
            !cold && i == 1,
        )?;
    }
    for i in 0..18 {
        let body = format!(
            "Alder unit {i} completed {} controlled checks and requires operator approval.",
            41 + i
        );
        let mut article = simple(
            &format!("short-{i}"),
            FEEDS[i % 5].0,
            body,
            20 - i as i64 / 2,
            !cold && i % 2 == 0,
        );
        article.cluster = (i < 4).then_some(7100 + (i / 2) as i64);
        fixture.insert(&article)?;
    }
    if name == "polluted" {
        for i in 0..35 {
            let article = simple(
                &format!("flood-{i}"),
                "cedar",
                format!("Cedar bulletin {i} lists a routine build log with no material change."),
                21,
                true,
            );
            fixture.insert(&article)?;
        }
        // Noise is computed by the production classifier, not a synthetic database column.
        for i in 0..12 {
            fixture.insert(&simple(
                &format!("noise-hidden-{i}"),
                "juniper",
                "An empty roundup without concrete information.".into(),
                -20,
                false,
            ))?;
        }
        for i in 0..8 {
            fixture.insert(&simple(
                &format!("noise-visible-{i}"),
                "juniper",
                format!("Juniper routine note {i} contains a short community poll."),
                if i < 2 { 46 - i as i64 } else { 25 },
                i >= 2,
            ))?;
        }
        for i in 0..2 {
            let mut read = simple(
                &format!("read-{i}"),
                "willow",
                "Willow inspection found 19 faults.".into(),
                44 - i,
                false,
            );
            read.read = true;
            fixture.insert(&read)?;
            let mut down = simple(
                &format!("down-{i}"),
                "fir",
                "Fir maintenance log records 28 routine retries.".into(),
                42 - i,
                false,
            );
            down.down = true;
            fixture.insert(&down)?;
        }
        let mut down = simple(
            "down-summarized",
            "fir",
            "Fir rejects a faulty adapter batch after 17 failures.".into(),
            39,
            true,
        );
        down.down = true;
        fixture.insert(&down)?;
        let mut read = simple(
            "read-summarized",
            "willow",
            "Willow repairs 28 stations after a verified outage.".into(),
            38,
            true,
        );
        read.read = true;
        fixture.insert(&read)?;
        let input = corpus
            .articles
            .iter()
            .find(|a| a.id == "community-link")
            .context("missing link case")?;
        fixture.from_corpus(input, "link-only", "alder", 40, None, false)?;
        let mut old = simple(
            "old-reingested",
            "maple",
            "Maple published this advisory thirty days ago.".into(),
            50,
            true,
        );
        old.age_hours = 30 * 24;
        let id = fixture.insert(&old)?;
        fixture.conn.execute(
            "UPDATE articles SET created_at=?1 WHERE id=?2",
            [NOW - 3_600_000, id],
        )?;
    }
    fixture.refresh_clusters()?;
    Ok(fixture)
}

pub fn representatives(
    conn: &rusqlite::Connection,
    items: &[DigestItem],
    limit: usize,
) -> Result<Vec<DigestItem>> {
    let candidates = items
        .iter()
        .map(|item| DigestCandidate {
            article_id: item.article_id,
            title: item.title.clone(),
            summary: item.summary.clone(),
            cluster_id: item.cluster_id,
        })
        .collect();
    let kept = rag::digest_representatives(conn, candidates, limit)?;
    Ok(kept
        .into_iter()
        .filter_map(|item| {
            items
                .iter()
                .find(|i| i.article_id == item.article_id)
                .cloned()
        })
        .collect())
}

#[derive(Clone, Deserialize)]
pub struct DailyLine {
    pub text: String,
    pub articles: Vec<String>,
}

#[derive(Clone, Deserialize)]
pub struct Daily {
    pub days_ago: i64,
    pub items: Vec<DailyLine>,
}

#[derive(Clone, Deserialize)]
pub struct WeeklyCase {
    pub name: String,
    pub articles: Vec<Article>,
    pub daily: Vec<Daily>,
    pub cross_day_articles: Vec<String>,
    pub high_value_supplements: Vec<String>,
    pub avoid_articles: Vec<String>,
    pub item_min: usize,
    pub item_max: usize,
}

pub fn store_daily(fixture: &FixtureDb, daily: &Daily) -> Result<()> {
    let ids: Vec<i64> = daily
        .items
        .iter()
        .flat_map(|line| &line.articles)
        .map(|key| fixture.id(key))
        .collect::<Result<_>>()?;
    let numbered = ai_store::digest_items_by_ids(&fixture.conn, &ids)?;
    let items = daily
        .items
        .iter()
        .map(|line| {
            let refs: Vec<usize> = line
                .articles
                .iter()
                .map(|key| {
                    let id = fixture.id(key)?;
                    numbered
                        .iter()
                        .position(|item| item.article_id == id)
                        .map(|n| n + 1)
                        .context("missing daily source")
                })
                .collect::<Result<_>>()?;
            Ok(json!({"text": line.text, "refs": refs}))
        })
        .collect::<Result<Vec<_>>>()?;
    let draft = ai_store::prepare_digest_draft(
        &json!({"sections": [{"title":"Developments", "items": items}]}),
        &numbered,
        OutputLanguage::En,
    )?;
    ai_store::insert_digest(
        &fixture.conn,
        "daily",
        NOW - daily.days_ago * DAY,
        &draft.body,
        &draft.items_json,
    )?;
    Ok(())
}

pub fn weekly(case: &WeeklyCase) -> Result<FixtureDb> {
    let mut fixture = FixtureDb::new()?;
    for article in &case.articles {
        fixture.insert(article)?;
    }
    fixture.refresh_clusters()?;
    for daily in &case.daily {
        store_daily(&fixture, daily)?;
    }
    Ok(fixture)
}

pub fn full_week(corpus: &Corpus) -> Result<FixtureDb> {
    let mut fixture = FixtureDb::new()?;
    for (day, count) in [8, 9, 10, 11, 12, 10, 10].into_iter().enumerate() {
        let mut lines = Vec::new();
        for i in 0..count {
            let key = format!("day-{day}-event-{i}");
            let detail = format!(
                "Maple trial {day}-{i} verified {} devices and retained {} maintenance records. ",
                120 + i,
                330 + day
            );
            let mut article = simple(&key, FEEDS[(day + i) % 5].0, detail.repeat(5), 12, true);
            article.summary = Some(detail.repeat(4));
            article.age_hours = (7 - day) as i64 * 24 - 4;
            article.cluster = Some(if i == 0 {
                9500
            } else {
                9600 + (day * 20 + i) as i64
            });
            fixture.insert(&article)?;
            lines.push(DailyLine {
                text: detail.repeat(2),
                articles: vec![key],
            });
        }
        store_daily(
            &fixture,
            &Daily {
                days_ago: (7 - day) as i64,
                items: lines,
            },
        )?;
    }
    for i in 0..7 {
        let input = corpus
            .articles
            .iter()
            .find(|a| a.id == if i % 2 == 0 { "en-depth" } else { "zh-depth" })
            .context("missing supplementary corpus input")?;
        fixture.from_corpus(
            input,
            &format!("missed-{i}"),
            FEEDS[i % 5].0,
            30 - i as i64,
            (i == 1).then_some(9500),
            true,
        )?;
    }
    fixture.refresh_clusters()?;
    Ok(fixture)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_week_has_seventy_verified_daily_items_and_excludes_their_ids() {
        rrss_core::evaluation_clock::at_time(NOW, || {
            let corpus =
                Corpus::load(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
            let f = full_week(&corpus).unwrap();
            let daily = ai_store::week_daily_lines(&f.conn, NOW - 7 * DAY, NOW).unwrap();
            assert_eq!(daily.len(), 70);
            let mut window = ai_store::DigestWindow::for_period("weekly", NOW, NOW, 30);
            window.exclude_daily_before = Some(i64::MAX);
            let candidates = ai_store::digest_candidates(&f.conn, 30, &window).unwrap();
            let used: BTreeSet<_> = daily.iter().flat_map(|d| &d.article_ids).copied().collect();
            assert!(candidates.iter().all(|c| !used.contains(&c.article_id)));
            // 7 篇遗漏文章中，1 篇与本周日报同簇（cluster 9500）被自动去重，留下 6 篇。
            assert_eq!(candidates.len(), 6);
        });
    }
}
