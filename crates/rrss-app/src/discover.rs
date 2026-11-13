//! Bounded, read-only feed discovery. Only declared links and same-origin paths are fetched.
use crate::{
    network::NetworkClients,
    views::{
        DiscoverFeedsError as Error, DiscoverFeedsRequest as Request,
        DiscoverFeedsResult as Outcome, DiscoveredFeed,
    },
};
use rrss_core::{
    ingest::{
        FetchRequest,
        feed_http::{self, FeedPolicy},
    },
    parse::parse_feed_bytes,
    url_norm::normalize_feed_url,
};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Mutex,
    time::Duration,
};
use tokio::sync::watch;
use url::Url;

const PROBE_PATHS: &[&str] = &["/feed", "/rss", "/atom.xml", "/index.xml", "/feed.xml"];
const MAX_CANDIDATE_URLS: usize = 6;
const MAX_REQUESTS: usize = 12; // Includes the input and every redirect hop.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(20);
const SAMPLE_TITLES: usize = 3;

#[derive(Default)]
pub struct Runtime(Mutex<HashMap<String, watch::Sender<()>>>);

struct Active<'a>(&'a Runtime, String, watch::Sender<()>);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        let mut active = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
        if active
            .get(&self.1)
            .is_some_and(|sender| sender.same_channel(&self.2))
        {
            active.remove(&self.1);
        }
    }
}

impl Runtime {
    pub async fn run(
        &self,
        network: &NetworkClients,
        db_path: &Path,
        request: Request,
    ) -> Result<Outcome, Error> {
        let (id, url) = match request {
            Request::Start { request_id, url } => (request_id, url),
            Request::Cancel { request_id } => {
                if let Some(sender) = self
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&request_id)
                {
                    sender.send_replace(());
                }
                return Ok(Outcome::Cancelled);
            }
        };
        let (sender, mut cancel) = watch::channel(());
        if let Some(old) = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), sender.clone())
        {
            old.send_replace(());
        }
        let _active = Active(self, id, sender);
        tokio::select! {
            biased;
            _ = cancel.changed() => Ok(Outcome::Cancelled),
            result = tokio::time::timeout(TOTAL_TIMEOUT, discover(network, db_path, &url)) =>
                result.unwrap_or(Err(Error::PageUnavailable)),
        }
    }
}

async fn discover(network: &NetworkClients, db_path: &Path, input: &str) -> Result<Outcome, Error> {
    let input = normalize_feed_url(input).map_err(|_| Error::PageUnavailable)?;
    let (subscribed, rsshub) = crate::jobs::spawn_db(db_path.to_path_buf(), |conn| {
        let mut statement = conn.prepare("SELECT url FROM feeds")?;
        let urls = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let subscribed: HashSet<String> = urls
            .iter()
            .filter_map(|url| normalize_feed_url(url).ok())
            .collect();
        let rsshub =
            conn.query_row("SELECT EXISTS(SELECT 1 FROM rsshub_instances)", [], |row| {
                row.get::<_, bool>(0)
            })?;
        Ok((subscribed, rsshub))
    })
    .await
    .map_err(|_| Error::PageUnavailable)?;
    // feed_client is Content scope with feed_http's explicit redirect policy.
    let client = network
        .feed_client()
        .await
        .map_err(|_| Error::PageUnavailable)?;
    let mut requests = 0;
    let (page_url, body) = download(&client, &input, &mut requests).await?;
    if let Some(mut candidate) = candidate(&page_url, &body, &subscribed) {
        candidate.subscribed |= subscribed.contains(&input);
        return Ok(Outcome::Candidates {
            candidates: vec![candidate],
        });
    }
    let html = crate::fulltext::decode_page(&body, "");
    if crate::fulltext::detect_waf(&html, 200) {
        return Err(Error::SiteProtection);
    }
    let declared = alternate_links(&html, &page_url);
    // Step 3: platform URL rules will contribute candidates here, before fallback probes.
    let probes = PROBE_PATHS
        .iter()
        .filter_map(|path| page_url.join(path).ok())
        .map(String::from)
        .collect();
    let mut seen = HashSet::from([input, page_url.to_string()]);
    let mut final_urls = HashSet::new();
    let mut candidates: Vec<DiscoveredFeed> = Vec::new();
    let mut failure = Error::NoFeed;
    // All sources share this single validation pass; fallback runs only if declarations yield no feed.
    for urls in [declared, probes] {
        if !candidates.is_empty() {
            break;
        }
        for url in urls {
            if requests >= MAX_REQUESTS {
                break;
            }
            if !seen.insert(url.clone()) {
                continue;
            }
            match download(&client, &url, &mut requests).await {
                Ok((final_url, body)) => {
                    let normalized = normalize_feed_url(final_url.as_str())
                        .map_err(|_| Error::PageUnavailable)?;
                    seen.insert(normalized.clone());
                    if !final_urls.insert(normalized.clone()) {
                        for candidate in &mut candidates {
                            if candidate.url == normalized {
                                candidate.subscribed |= subscribed.contains(&url);
                            }
                        }
                        continue;
                    }
                    if let Some(mut found) = candidate(&final_url, &body, &subscribed) {
                        found.subscribed |= subscribed.contains(&url);
                        candidates.push(found);
                    } else if crate::fulltext::detect_waf(
                        &crate::fulltext::decode_page(&body, ""),
                        200,
                    ) {
                        failure = Error::SiteProtection;
                    }
                }
                Err(Error::SiteProtection) => {
                    failure = Error::SiteProtection;
                }
                Err(_) => {}
            }
        }
    }
    if !candidates.is_empty() {
        return Ok(Outcome::Candidates { candidates });
    }
    if failure == Error::SiteProtection {
        return Err(failure);
    }
    if rsshub {
        Ok(Outcome::RsshubAvailable)
    } else {
        Err(failure)
    }
}

fn alternate_links(html: &str, page: &Url) -> Vec<String> {
    let dom = dom_query::Document::from(html);
    let mut seen = HashSet::new();
    dom.select("link[href]")
        .iter()
        .filter_map(|link| {
            if !link
                .attr("rel")?
                .split_ascii_whitespace()
                .any(|rel| rel.eq_ignore_ascii_case("alternate"))
            {
                return None;
            }
            let mime = link.attr("type")?;
            let mime = mime.split(';').next()?.trim().to_ascii_lowercase();
            if !matches!(
                mime.as_str(),
                "application/rss+xml"
                    | "application/atom+xml"
                    | "application/feed+json"
                    | "application/json"
            ) {
                return None;
            }
            let url = page.join(link.attr("href")?.trim()).ok()?;
            let url = normalize_feed_url(url.as_str()).ok()?;
            seen.insert(url.clone()).then_some(url)
        })
        .take(MAX_CANDIDATE_URLS)
        .collect()
}

fn candidate(url: &Url, body: &[u8], subscribed: &HashSet<String>) -> Option<DiscoveredFeed> {
    let parsed = parse_feed_bytes(body, Some(url.as_str())).ok()?;
    let url = normalize_feed_url(url.as_str()).ok()?;
    Some(DiscoveredFeed {
        subscribed: subscribed.contains(&url),
        url,
        title: parsed.title,
        feed_type: parsed.wire_kind.as_str().into(),
        updated_at: parsed.updated_at,
        entry_count: parsed.entries.len(),
        sample_titles: parsed
            .entries
            .into_iter()
            .take(SAMPLE_TITLES)
            .map(|entry| entry.title)
            .collect(),
    })
}

fn redirect_allowed(from: &Url, to: &Url) -> bool {
    let same_host = match (from.host_str(), to.host_str()) {
        (Some(a), Some(b)) => {
            a.strip_prefix("www.").unwrap_or(a) == b.strip_prefix("www.").unwrap_or(b)
        }
        _ => false,
    };
    same_host
        && matches!(
            (from.scheme(), to.scheme()),
            ("http", "http" | "https") | ("https", "https")
        )
}

async fn download(
    client: &reqwest::Client,
    input: &str,
    requests: &mut usize,
) -> Result<(Url, Vec<u8>), Error> {
    let mut previous = Url::parse(input).map_err(|_| Error::PageUnavailable)?;
    let mut final_url = previous.clone();
    let response = feed_http::fetch_controlled(
        client,
        FetchRequest {
            url: input.into(),
            etag: None,
            last_modified: None,
        },
        FeedPolicy {
            timeout: REQUEST_TIMEOUT.min(feed_http::TOTAL_TIMEOUT),
            ..FeedPolicy::default()
        },
        |url| {
            let allowed = redirect_allowed(&previous, &url) && *requests < MAX_REQUESTS;
            if allowed {
                *requests += 1;
                previous = url;
            }
            async move {
                if allowed {
                    Ok(())
                } else {
                    Err(rrss_core::Error::Parse("discovery_request_boundary".into()))
                }
            }
        },
        |_| {},
        |head| {
            final_url = head.url;
            async { Ok(()) }
        },
    )
    .await
    .map_err(|_| Error::PageUnavailable)?;
    if crate::fulltext::detect_waf("", response.status) || response.status == 503 {
        return Err(Error::SiteProtection);
    }
    if !(200..300).contains(&response.status) {
        return Err(Error::PageUnavailable);
    }
    Ok((final_url, response.body))
}

#[cfg(test)]
mod tests;
