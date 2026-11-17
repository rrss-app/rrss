//! Plan whole-window clustering on a read snapshot, then publish the validated delta.
use super::*;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Serialize)]
struct Article {
    id: i64,
    feed: i64,
    title: String,
    url: Option<String>,
    fingerprint: Option<i64>,
    published: Option<i64>,
    created: i64,
    score: i64,
    cluster: Option<i64>,
    html: Option<String>,
}
impl Article {
    fn time(&self) -> i64 {
        self.published.unwrap_or(self.created)
    }
}

#[derive(Clone, PartialEq, Serialize)]
struct Group {
    id: i64,
    label: String,
    representative: Option<i64>,
    count: i64,
    first: Option<i64>,
    last: Option<i64>,
    created: i64,
}

struct Inputs {
    articles: BTreeMap<i64, Article>,
    groups: BTreeMap<i64, Group>,
    vectors: BTreeMap<i64, Vec<f32>>,
}
impl Inputs {
    fn load(conn: &Connection, window: i64, rebuild: bool) -> Result<Self> {
        let articles = conn.prepare("SELECT id,feed_id,title,canonical_url,simhash,published_at,created_at,score,cluster_id,
            CASE WHEN ?2 AND COALESCE(published_at,created_at)>=?1 THEN content_html END
            FROM articles WHERE expires_at IS NULL ORDER BY id")?
            .query_map(params![window,rebuild], |r| { let article = Article {
                id:r.get(0)?,feed:r.get(1)?,title:r.get(2)?,url:r.get(3)?,fingerprint:r.get(4)?,
                published:r.get(5)?,created:r.get(6)?,score:r.get(7)?,cluster:r.get(8)?,html:r.get(9)?,
            }; Ok((article.id,article)) })?.collect::<rusqlite::Result<_>>()?;
        let groups = conn.prepare("SELECT id,label,representative_id,member_count,first_published_at,last_published_at,created_at FROM clusters ORDER BY id")?
            .query_map([], |r| { let group=Group { id:r.get(0)?,label:r.get(1)?,representative:r.get(2)?,
                count:r.get(3)?,first:r.get(4)?,last:r.get(5)?,created:r.get(6)? }; Ok((group.id,group)) })?
            .collect::<rusqlite::Result<_>>()?;
        let vectors = rag::load_selected_article_vectors_since(conn, i64::MIN, window)?
            .into_iter()
            .filter_map(|a| a.vector.map(|v| (a.article_id, v)))
            .collect();
        Ok(Self {
            articles,
            groups,
            vectors,
        })
    }
}

struct Graph {
    articles: BTreeMap<i64, Article>,
    groups: BTreeMap<i64, Group>,
    members: BTreeMap<i64, BTreeSet<i64>>,
    vectors: BTreeMap<i64, Vec<f32>>,
    next: i64,
    now: i64,
    cleared: BTreeSet<i64>,
}
impl Graph {
    fn new(inputs: &mut Inputs, now: i64) -> Self {
        let mut members: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
        for a in inputs.articles.values() {
            if let Some(cid) = a.cluster {
                members.entry(cid).or_default().insert(a.id);
            }
        }
        Self {
            articles: inputs.articles.clone(),
            groups: inputs.groups.clone(),
            members,
            vectors: std::mem::take(&mut inputs.vectors),
            next: inputs.groups.keys().next_back().copied().unwrap_or(0) + 1,
            now,
            cleared: BTreeSet::new(),
        }
    }
    fn refresh(&mut self, cid: i64) {
        let ids = self.members.get(&cid).cloned().unwrap_or_default();
        let Some(group) = self.groups.get_mut(&cid) else {
            return;
        };
        let representative = ids
            .iter()
            .map(|id| &self.articles[id])
            .max_by_key(|a| (a.score, a.published.unwrap_or(0), a.id));
        group.count = ids.len() as i64;
        group.representative = representative.map(|a| a.id);
        group.first = ids
            .iter()
            .filter_map(|id| self.articles[id].published)
            .min();
        group.last = ids
            .iter()
            .filter_map(|id| self.articles[id].published)
            .max();
        if let Some(a) = representative {
            group.label = truncate_chars(&a.title, LABEL_CHARS);
        }
    }
    fn create(&mut self, id: i64) -> i64 {
        let cid = self.next;
        self.next += 1;
        let article = &mut self.articles.get_mut(&id).expect("known article");
        article.cluster = Some(cid);
        self.groups.insert(
            cid,
            Group {
                id: cid,
                label: truncate_chars(&article.title, LABEL_CHARS),
                representative: Some(id),
                count: 1,
                first: Some(article.time()),
                last: Some(article.time()),
                created: self.now,
            },
        );
        self.members.insert(cid, BTreeSet::from([id]));
        cid
    }
    fn join(&mut self, id: i64, cid: i64) {
        self.articles.get_mut(&id).unwrap().cluster = Some(cid);
        self.members.entry(cid).or_default().insert(id);
        self.refresh(cid);
    }
    fn source_compatible(&self, a: i64, b: i64) -> bool {
        let mut ids = BTreeSet::from([a, b]);
        for id in [a, b] {
            if let Some(cid) = self.articles[&id].cluster {
                if let Some(members) = self.members.get(&cid) {
                    ids.extend(members);
                }
            }
        }
        let mut sources: HashMap<i64, Option<&str>> = HashMap::new();
        for id in ids {
            let item = &self.articles[&id];
            let url = item.url.as_deref().filter(|s| !s.is_empty());
            if let Some(previous) = sources.insert(item.feed, url) {
                if previous.is_none() || url.is_none() || previous != url {
                    return false;
                }
            }
        }
        true
    }
    fn assign(&mut self, id: i64) {
        let article = self.articles[&id].clone();
        if let Some(cid) = article.cluster {
            self.refresh(cid);
            return;
        }
        if let Some(url) = article.url.as_deref().filter(|u| !u.is_empty()) {
            let mut candidates: Vec<_> = self
                .articles
                .values()
                .filter(|a| a.id != id && a.url.as_deref() == Some(url))
                .map(|a| (a.cluster.is_none(), a.id))
                .collect();
            candidates.sort_unstable();
            for (_, other) in candidates {
                if self.source_compatible(id, other) {
                    let cid = self.articles[&other]
                        .cluster
                        .unwrap_or_else(|| self.create(other));
                    self.join(id, cid);
                    return;
                }
            }
        }
        if let Some(fp) = article.fingerprint.filter(|s| *s != 0) {
            let start = article.time() - CLUSTER_WINDOW_DAYS * MS_PER_DAY;
            let mut matches: Vec<_> = self
                .articles
                .values()
                .filter(|a| a.id != id && a.time() >= start)
                .filter_map(|a| {
                    a.fingerprint
                        .filter(|s| *s != 0)
                        .map(|other| (hamming(fp, other), a.id))
                })
                .filter(|(d, _)| *d <= SIMHASH_HAMMING)
                .collect();
            matches.sort_unstable();
            for (_, other) in matches {
                if self.source_compatible(id, other) {
                    let cid = self.articles[&other]
                        .cluster
                        .unwrap_or_else(|| self.create(other));
                    self.join(id, cid);
                    return;
                }
            }
        }
        self.create(id);
    }
    fn rebuild(&mut self, window: i64) {
        let ids: Vec<_> = self
            .articles
            .values()
            .filter(|a| a.time() >= window)
            .map(|a| a.id)
            .collect();
        let mut old = BTreeSet::new();
        for id in &ids {
            let article = self.articles.get_mut(id).unwrap();
            if let Some(cid) = article.cluster.take() {
                old.insert(cid);
                self.members.entry(cid).or_default().remove(id);
            }
            article.fingerprint =
                Some(fingerprint_article(&article.title, article.html.as_deref()));
        }
        for cid in old {
            self.refresh(cid);
            if self.members.get(&cid).is_none_or(|m| m.is_empty()) {
                self.cleared.insert(cid);
            }
        }
        for id in ids {
            self.assign(id);
        }
    }
    fn merge(&mut self, window: i64, threshold: f32) -> u32 {
        let reps: Vec<_> = self
            .groups
            .values()
            .filter(|g| {
                g.representative
                    .is_some_and(|id| self.vectors.contains_key(&id))
                    && self.members.get(&g.id).is_some_and(|m| {
                        !m.is_empty() && m.iter().all(|id| self.articles[id].time() >= window)
                    })
            })
            .map(|g| g.id)
            .collect();
        let mut merged = 0;
        for (i, &a) in reps.iter().enumerate() {
            if !self.groups.contains_key(&a) {
                continue;
            }
            for &b in &reps[i + 1..] {
                let Some(other) = self.groups.get(&b) else {
                    continue;
                };
                let (ra, rb) = (
                    self.groups[&a].representative.unwrap(),
                    other.representative.unwrap(),
                );
                let empty = Vec::new();
                let distance = cosine_distance(
                    self.vectors.get(&ra).unwrap_or(&empty),
                    self.vectors.get(&rb).unwrap_or(&empty),
                );
                if !distance.is_finite()
                    || 1.0 - distance <= threshold
                    || !titles_overlap(&self.articles[&ra].title, &self.articles[&rb].title)
                    || !self.source_compatible(ra, rb)
                {
                    continue;
                }
                let times: Vec<_> = self.members[&a]
                    .iter()
                    .chain(&self.members[&b])
                    .map(|id| self.articles[id].time())
                    .collect();
                if times.iter().max().unwrap() - times.iter().min().unwrap()
                    > EVENT_WINDOW_DAYS * MS_PER_DAY
                {
                    continue;
                }
                let (survivor, loser) = if self.members[&a].len() >= self.members[&b].len() {
                    (a, b)
                } else {
                    (b, a)
                };
                let moving = self.members.remove(&loser).unwrap();
                for id in &moving {
                    self.articles.get_mut(id).unwrap().cluster = Some(survivor);
                }
                self.members.entry(survivor).or_default().extend(moving);
                self.groups.remove(&loser);
                self.refresh(survivor);
                merged += 1;
                if loser == a {
                    break;
                }
            }
        }
        merged
    }
}

pub(super) fn run(conn: &Connection, rebuild: bool) -> Result<u32> {
    run_with(conn, rebuild, || Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_vectors_are_limited_to_window_and_moved_into_graph() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("window.db")).unwrap();
        crate::vector_space::seed_active(&conn, "fixture", 3);
        let space = crate::vector_space::active(&conn).unwrap().unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://cluster.example/feed',0);
            INSERT INTO articles(id,feed_id,title,created_at,published_at,expires_at) VALUES
            (1,1,'old',10,NULL,NULL),(2,1,'recent',100,NULL,NULL),(3,1,'expired',100,NULL,200);",
        )
        .unwrap();
        for id in 1..=3 {
            conn.execute("INSERT INTO chunks(article_id,ordinal,kind,text,embedding,space_id) VALUES(?1,0,'title_summary','fixture',?2,?3)",
                params![id, rag::f32s_to_le(&[1.0,0.0,0.0]), space.id]).unwrap();
        }
        let mut inputs = Inputs::load(&conn, 100, false).unwrap();
        assert_eq!(inputs.vectors.keys().copied().collect::<Vec<_>>(), vec![2]);
        let graph = Graph::new(&mut inputs, 100);
        assert!(inputs.vectors.is_empty());
        assert_eq!(graph.vectors[&2], vec![1.0, 0.0, 0.0]);
    }
}

pub(super) fn run_with(
    conn: &Connection,
    rebuild: bool,
    between: impl Fn() -> Result<()>,
) -> Result<u32> {
    for _ in 0..3 {
        if rebuild
            && crate::db::get_setting(conn, ALGORITHM_VERSION_KEY)?.as_deref()
                == Some(ALGORITHM_VERSION)
        {
            return Ok(0);
        }
        let now = now_ms();
        let window = now - CLUSTER_WINDOW_DAYS * MS_PER_DAY;
        let (before, signature, graph, calibration, merged) =
            crate::db::read_snapshot(conn, |conn| {
                let mut before = Inputs::load(conn, window, rebuild)?;
                let signature = crate::db::input_revision(conn, "cluster")?;
                let calibration = calibrate(conn, window)?;
                let mut graph = Graph::new(&mut before, now);
                if rebuild {
                    graph.rebuild(window);
                }
                let merged = graph.merge(window, calibration.threshold);
                Ok((before, signature, graph, calibration, merged))
            })?;
        if !rebuild && merged == 0 && calibration.cache.is_none() {
            return Ok(0);
        }
        // Encode the write set before acquiring the writer lock.
        let articles: Vec<_> = graph
            .articles
            .values()
            .filter(|a| {
                let old = &before.articles[&a.id];
                old.cluster != a.cluster || old.fingerprint != a.fingerprint
            })
            .map(|a| serde_json::json!([a.id, a.cluster, a.fingerprint]))
            .collect();
        let added: Vec<_> = graph
            .groups
            .values()
            .filter(|g| !before.groups.contains_key(&g.id))
            .collect();
        let changed: Vec<_> = graph
            .groups
            .values()
            .filter(|g| before.groups.get(&g.id).is_some_and(|old| old != *g))
            .collect();
        let removed: Vec<_> = before
            .groups
            .keys()
            .filter(|id| !graph.groups.contains_key(id))
            .collect();
        let articles = serde_json::to_string(&articles).unwrap();
        let added = serde_json::to_string(&added).unwrap();
        let changed = serde_json::to_string(&changed).unwrap();
        let removed = serde_json::to_string(&removed).unwrap();
        let cleared = serde_json::to_string(&graph.cleared).unwrap();
        between()?;
        let committed = crate::db::atomic_write(conn, |conn| {
            if crate::db::input_revision(conn, "cluster")? != signature {
                return Ok(false);
            }
            conn.execute("INSERT INTO clusters(id,label,representative_id,member_count,first_published_at,last_published_at,created_at)
                SELECT json_extract(value,'$.id'),json_extract(value,'$.label'),json_extract(value,'$.representative'),
                json_extract(value,'$.count'),json_extract(value,'$.first'),json_extract(value,'$.last'),json_extract(value,'$.created') FROM json_each(?1)", [&added])?;
            conn.execute("UPDATE articles SET cluster_id=json_extract(j.value,'$[1]'),simhash=json_extract(j.value,'$[2]')
                FROM json_each(?1) j WHERE articles.id=json_extract(j.value,'$[0]')", [&articles])?;
            conn.execute("UPDATE clusters SET label=json_extract(j.value,'$.label'),representative_id=json_extract(j.value,'$.representative'),
                member_count=json_extract(j.value,'$.count'),first_published_at=json_extract(j.value,'$.first'),last_published_at=json_extract(j.value,'$.last')
                FROM json_each(?1) j WHERE clusters.id=json_extract(j.value,'$.id')", [&changed])?;
            conn.execute(
                "DELETE FROM clusters WHERE id IN (SELECT value FROM json_each(?1))",
                [&removed],
            )?;
            conn.execute("UPDATE clusters SET summary_json=NULL,summary_members=NULL,summary_at=NULL WHERE id IN (SELECT value FROM json_each(?1))", [&cleared])?;
            calibration.persist(conn)?;
            if rebuild {
                crate::db::set_setting(conn, ALGORITHM_VERSION_KEY, ALGORITHM_VERSION)?;
            }
            Ok(true)
        })?;
        if committed {
            return Ok(merged);
        }
    }
    Err(
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY), None)
            .into(),
    )
}
