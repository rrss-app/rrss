use super::*;
use rrss_core::{
    db, feeds,
    network::{self, ProxyConfig, ProxyRule},
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

const HOME: &str = include_str!("../../../../testdata/feeds/discovery/home.html");
const ARTICLE: &str = include_str!("../../../../testdata/feeds/discovery/article.html");
const PROFILE: &str = include_str!("../../../../testdata/feeds/discovery/profile.html");
const RELATIVE: &str = include_str!("../../../../testdata/feeds/discovery/relative.html");
const EMPTY: &str = include_str!("../../../../testdata/feeds/discovery/empty.html");
const PROTECTED: &str = include_str!("../../../../testdata/feeds/discovery/protected.html");
const REDIRECT: &str = include_str!("../../../../testdata/feeds/discovery/redirect.json");
const RSS: &str = include_str!("../../../../testdata/feeds/discovery/rss.xml");
const ATOM: &str = include_str!("../../../../testdata/feeds/discovery/atom.xml");
const JSON: &str = include_str!("../../../../testdata/feeds/discovery/feed.json");
const PAGE: &str = "http://journal.example";

struct Site {
    server: MockServer,
    unused: MockServer,
    _dir: tempfile::TempDir,
    db: std::path::PathBuf,
    network: NetworkClients,
    runtime: Runtime,
}
impl Site {
    async fn new() -> Self {
        let server = MockServer::start().await;
        let unused = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("discovery.db");
        let conn = db::open_at(&db).unwrap();
        // All fixture domains resolve through the Content proxy; other scopes must stay unused.
        network::save(
            &conn,
            &ProxyConfig {
                global: ProxyRule::custom(&unused.uri()),
                content: ProxyRule::custom(&server.uri()),
                api: ProxyRule::custom(&unused.uri()),
                pi: ProxyRule::custom(&unused.uri()),
                ..Default::default()
            },
        )
        .unwrap();
        Self {
            network: NetworkClients::new(&db),
            server,
            unused,
            _dir: dir,
            db,
            runtime: Runtime::default(),
        }
    }
    async fn serve(&self, route: &str, body: &'static str) {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .expect(1)
            .mount(&self.server)
            .await;
    }
    async fn run(&self, route: &str) -> Result<Outcome, Error> {
        self.runtime
            .run(
                &self.network,
                &self.db,
                Request::Start {
                    request_id: "fixture".into(),
                    url: format!("{PAGE}{route}"),
                },
            )
            .await
    }
    async fn requests(&self, count: usize) {
        assert_eq!(self.server.received_requests().await.unwrap().len(), count);
        assert!(self.unused.received_requests().await.unwrap().is_empty());
    }
}
fn candidates(outcome: Outcome) -> Vec<DiscoveredFeed> {
    let Outcome::Candidates { candidates } = outcome else {
        panic!("expected candidates: {outcome:?}")
    };
    candidates
}

#[tokio::test]
async fn homepage_uses_content_scope_and_only_explicit_links() {
    let site = Site::new().await;
    site.serve("/", HOME).await;
    site.serve("/news.xml", RSS).await;
    let found = candidates(site.run("/").await.unwrap());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].url, "http://syndication.example/news.xml");
    assert_eq!(found[0].title.as_deref(), Some("Lantern Journal"));
    assert_eq!(found[0].feed_type, "rss");
    assert_eq!(found[0].updated_at, Some(1794484800000));
    assert_eq!(found[0].entry_count, 4);
    assert_eq!(
        found[0].sample_titles,
        ["Paper observatories", "Stone notebooks", "Quiet gardens"]
    );
    assert!(!found[0].subscribed);
    site.requests(2).await;
}

#[tokio::test]
async fn article_discovers_atom_and_uses_latest_entry_update() {
    let site = Site::new().await;
    site.serve("/posts/paper", ARTICLE).await;
    site.serve("/journal.atom", ATOM).await;
    let found = candidates(site.run("/posts/paper").await.unwrap());
    assert_eq!(found[0].feed_type, "atom");
    assert_eq!(found[0].updated_at, Some(1794484800000));
    site.requests(2).await;
}

#[tokio::test]
async fn personal_profile_discovers_json_feed() {
    let site = Site::new().await;
    site.serve("/rowan", PROFILE).await;
    site.serve("/rowan/feed.json", JSON).await;
    let found = candidates(site.run("/rowan").await.unwrap());
    assert_eq!(found[0].feed_type, "json");
    assert_eq!(found[0].sample_titles, ["Small discoveries"]);
    assert_eq!(found[0].updated_at, Some(1794484800000));
    site.requests(2).await;
}

#[tokio::test]
async fn feed_input_returns_immediately_without_probing() {
    let site = Site::new().await;
    site.serve("/already.xml", RSS).await;
    assert_eq!(candidates(site.run("/already.xml").await.unwrap()).len(), 1);
    site.requests(1).await;
}

#[tokio::test]
async fn missing_feed_probes_only_the_fixed_shortlist() {
    let site = Site::new().await;
    site.serve("/empty", EMPTY).await;
    assert_eq!(site.run("/empty").await.unwrap_err(), Error::NoFeed);
    let requests = site.server.received_requests().await.unwrap();
    assert!(PROBE_PATHS.len() <= 6);
    assert_eq!(
        requests
            .iter()
            .skip(1)
            .map(|r| r.url.path())
            .collect::<Vec<_>>(),
        PROBE_PATHS
    );
    assert!(
        requests
            .iter()
            .all(|r| r.url.host_str() == Some("journal.example"))
    );
    site.requests(1 + PROBE_PATHS.len()).await;
}

#[tokio::test]
async fn protection_pages_and_statuses_stop_before_probing() {
    let site = Site::new().await;
    for status in [200, 403, 429, 503] {
        let route = format!("/protected-{status}");
        Mock::given(path(route.clone()))
            .respond_with(ResponseTemplate::new(status).set_body_string(PROTECTED))
            .expect(1)
            .mount(&site.server)
            .await;
        assert_eq!(site.run(&route).await.unwrap_err(), Error::SiteProtection);
    }
    site.requests(4).await;
}

#[tokio::test]
async fn redirect_resolves_links_against_the_final_page() {
    let site = Site::new().await;
    let redirect: serde_json::Value = serde_json::from_str(REDIRECT).unwrap();
    Mock::given(path("/old"))
        .respond_with(
            ResponseTemplate::new(redirect["status"].as_u64().unwrap() as u16)
                .insert_header("location", redirect["location"].as_str().unwrap()),
        )
        .expect(1)
        .mount(&site.server)
        .await;
    site.serve("/journal/posts/", RELATIVE).await;
    site.serve("/journal/news.xml", RSS).await;
    let found = candidates(site.run("/old").await.unwrap());
    assert_eq!(found[0].url, format!("{PAGE}/journal/news.xml"));
    site.requests(3).await;
}

#[tokio::test]
async fn relative_links_ignore_base_and_deduplicate_before_validation() {
    let site = Site::new().await;
    site.serve("/journal/posts/", RELATIVE).await;
    site.serve("/journal/news.xml", RSS).await;
    assert_eq!(
        candidates(site.run("/journal/posts/").await.unwrap()).len(),
        1
    );
    site.requests(2).await;
}

#[tokio::test]
async fn subscribed_marker_uses_existing_normalization_without_writing() {
    let site = Site::new().await;
    let conn = db::open_at(&site.db).unwrap();
    feeds::subscribe(
        &conn,
        feeds::SubscribeRequest {
            url: "http://JOURNAL.example:80/journal.atom?utm_source=old#top".into(),
            folder_id: None,
            as_rsshub: false,
            rsshub_instance_id: None,
        },
    )
    .unwrap();
    site.serve("/posts/paper", ARTICLE).await;
    site.serve("/journal.atom", ATOM).await;
    assert!(candidates(site.run("/posts/paper").await.unwrap())[0].subscribed);
    assert_eq!(feeds::list_feeds(&conn).unwrap().len(), 1);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM articles", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    site.requests(2).await;
}

#[tokio::test]
async fn invalid_declared_feed_falls_back_and_hints_do_not_override_parsed_type() {
    let site = Site::new().await;
    site.serve("/posts/paper", ARTICLE).await;
    site.serve("/journal.atom", EMPTY).await;
    site.serve("/rss", JSON).await;
    let found = candidates(site.run("/posts/paper").await.unwrap());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].feed_type, "json");
    site.requests(2 + PROBE_PATHS.len()).await;
}

#[tokio::test]
async fn rsshub_is_only_a_hint_and_never_receives_requests() {
    let site = Site::new().await;
    db::open_at(&site.db)
        .unwrap()
        .execute(
            "INSERT INTO rsshub_instances (name, base_url, created_at)
             VALUES ('Bridge Studio', 'http://bridge.example/', 0)",
            [],
        )
        .unwrap();
    site.serve("/empty", EMPTY).await;
    assert!(matches!(
        site.run("/empty").await.unwrap(),
        Outcome::RsshubAvailable
    ));
    site.requests(1 + PROBE_PATHS.len()).await;
    assert!(
        site.server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.url.host_str() == Some("journal.example"))
    );
}

#[tokio::test]
async fn unavailable_and_unrelated_redirects_return_codes_without_requesting_other_sites() {
    let site = Site::new().await;
    Mock::given(path("/away"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "http://unrelated.example/"),
        )
        .expect(1)
        .mount(&site.server)
        .await;
    for route in ["/missing", "/away"] {
        assert_eq!(site.run(route).await.unwrap_err(), Error::PageUnavailable);
    }
    site.requests(2).await;
}

#[tokio::test]
async fn redirect_hops_share_the_request_budget() {
    let site = Site::new().await;
    site.serve("/empty", EMPTY).await;
    // Every fallback path loops; the global budget also bounds redirect requests.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/loop"))
        .with_priority(10)
        .mount(&site.server)
        .await;
    assert_eq!(site.run("/empty").await.unwrap_err(), Error::NoFeed);
    site.requests(MAX_REQUESTS).await;
}

#[tokio::test]
async fn declarations_are_capped_even_when_the_page_lists_many_feeds() {
    let site = Site::new().await;
    let html: String = (0..20)
        .map(|i| format!("<link rel='alternate' type='application/rss+xml' href='/candidate-{i}'>"))
        .collect();
    Mock::given(path("/many"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html))
        .expect(1)
        .mount(&site.server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ATOM))
        .with_priority(10)
        .mount(&site.server)
        .await;
    assert_eq!(
        candidates(site.run("/many").await.unwrap()).len(),
        MAX_CANDIDATE_URLS
    );
    site.requests(1 + MAX_CANDIDATE_URLS).await;
}

#[tokio::test]
async fn replacing_and_cancelling_a_request_drops_downloads_and_cleans_runtime() {
    let site = Site::new().await;
    let started = std::sync::Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    Mock::given(path("/slow"))
        .respond_with(move |_: &wiremock::Request| {
            signal.notify_one();
            ResponseTemplate::new(200)
                .set_body_string(RSS)
                .set_delay(Duration::from_secs(60))
        })
        .expect(2)
        .mount(&site.server)
        .await;
    let pending = site.run("/slow");
    let replace = async {
        started.notified().await;
        let cancel = async {
            started.notified().await;
            site.runtime
                .run(
                    &site.network,
                    &site.db,
                    Request::Cancel {
                        request_id: "fixture".into(),
                    },
                )
                .await
                .unwrap()
        };
        tokio::join!(site.run("/slow"), cancel)
    };
    let (result, (replacement, cancelled)) = tokio::join!(pending, replace);
    assert!(matches!(result.unwrap(), Outcome::Cancelled));
    assert!(matches!(replacement.unwrap(), Outcome::Cancelled));
    assert!(matches!(cancelled, Outcome::Cancelled));
    assert!(site.runtime.0.lock().unwrap().is_empty());
    site.requests(2).await;
}

#[tokio::test]
async fn total_deadline_covers_multiple_individually_bounded_requests() {
    let site = Site::new().await;
    site.serve("/empty", EMPTY).await;
    let (signal, mut received) = tokio::sync::mpsc::unbounded_channel();
    Mock::given(method("GET"))
        .respond_with(move |_: &wiremock::Request| {
            signal.send(()).unwrap();
            ResponseTemplate::new(200)
                .set_body_string(RSS)
                .set_delay(Duration::from_secs(60))
        })
        .with_priority(10)
        .mount(&site.server)
        .await;
    let pending = site.run("/empty");
    let advance = async {
        for _ in 0..2 {
            tokio::time::timeout(REQUEST_TIMEOUT, received.recv())
                .await
                .unwrap()
                .unwrap();
            tokio::time::pause();
            tokio::time::advance(REQUEST_TIMEOUT).await;
            // Keep real socket I/O outside paused time; auto-advance would race the next request.
            tokio::time::resume();
        }
        tokio::time::timeout(REQUEST_TIMEOUT, received.recv())
            .await
            .unwrap()
            .unwrap();
        tokio::time::pause();
        tokio::time::advance(TOTAL_TIMEOUT - REQUEST_TIMEOUT * 2).await;
        tokio::time::resume();
    };
    let (result, ()) = tokio::join!(pending, advance);
    assert_eq!(result.unwrap_err(), Error::PageUnavailable);
    site.requests(4).await;
}

#[tokio::test]
async fn redirected_candidates_merge_subscription_aliases() {
    let site = Site::new().await;
    site.serve("/aliases", "<link rel='alternate' type='application/rss+xml' href='/first'><link rel='alternate' type='application/rss+xml' href='/subscribed'>").await;
    for route in ["/first", "/subscribed"] {
        Mock::given(path(route))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/final"))
            .expect(1)
            .mount(&site.server)
            .await;
    }
    Mock::given(path("/final"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ATOM))
        .expect(2)
        .mount(&site.server)
        .await;
    feeds::subscribe(
        &db::open_at(&site.db).unwrap(),
        feeds::SubscribeRequest {
            url: format!("{PAGE}/subscribed"),
            folder_id: None,
            as_rsshub: false,
            rsshub_instance_id: None,
        },
    )
    .unwrap();
    let found = candidates(site.run("/aliases").await.unwrap());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].url, format!("{PAGE}/final"));
    assert!(found[0].subscribed);
    site.requests(5).await;
}

#[test]
fn redirects_allow_site_aliases_and_upgrades_but_never_downgrades_or_other_hosts() {
    for (from, to, allowed) in [
        ("http://journal.example/", "https://journal.example/", true),
        (
            "https://journal.example/",
            "https://www.journal.example/",
            true,
        ),
        (
            "https://www.journal.example/",
            "https://journal.example/",
            true,
        ),
        ("https://journal.example/", "http://journal.example/", false),
        (
            "https://journal.example/",
            "https://unrelated.example/",
            false,
        ),
    ] {
        assert_eq!(
            redirect_allowed(&Url::parse(from).unwrap(), &Url::parse(to).unwrap()),
            allowed,
            "{from} -> {to}"
        );
    }
}
