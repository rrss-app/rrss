//! Original-page policy and session directories; no Tauri or native UI dependencies.
use crate::{UiError, ui_error::web::WebError};
use std::path::{Path, PathBuf};

use rrss_core::{
    db,
    network::{self, NetworkScope, ProxyEnvironment, WebviewRoute},
    platform::{self, Os, SystemProxySnapshot},
};
use rusqlite::Connection;
use url::Url;

pub struct WebWindowPlan {
    pub url: Url,
    pub title: String,
    pub route: WebviewRoute,
    pub javascript: bool,
}

pub fn title(_url: &Url) -> String {
    "web_original_title".into()
}

pub fn prepare(
    conn: &Connection,
    raw: &str,
    env: &ProxyEnvironment,
    native: &SystemProxySnapshot,
    os: Os,
) -> Result<WebWindowPlan, UiError> {
    let url = Url::parse(raw.trim()).map_err(|_| UiError::from(WebError::HttpOnly))?;
    if !rrss_core::url_norm::is_http_url(&url) {
        return Err(UiError::from(WebError::HttpOnly));
    }
    let site = crate::site_rules::resolve_site_rule(conn, url.host_str().unwrap())?;
    let javascript = db::get_setting(conn, "web_javascript")?.as_deref() != Some("0")
        && !site.rule.is_some_and(|r| r.javascript_disabled);
    Ok(WebWindowPlan {
        title: title(&url),
        url,
        javascript,
        route: network::load(conn)?.webview_route(NetworkScope::Content, env, native, os)?,
    })
}

pub struct WebSession {
    root: PathBuf,
    pub directory: PathBuf,
    serial: u64,
    route: Option<WebviewRoute>,
}

/// Only direct child directories are candidates; never follow a session symlink.
fn abandoned_directories(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    std::fs::read_dir(root)?
        .filter_map(|entry| match entry {
            Ok(entry) => match entry.file_type() {
                Ok(kind) if kind.is_dir() => Some(Ok(entry.path())),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            },
            Err(error) => Some(Err(error)),
        })
        .collect()
}

impl WebSession {
    /// Keep the main application usable even when browser storage is unavailable.
    pub fn initialize(data_dir: &Path, run_id: &str) -> Result<Self, UiError> {
        Self::start(data_dir, run_id).map_err(|error| {
            tracing::warn!(error_kind = ?error.kind(), "original-page data directory unavailable");
            WebError::SessionUnavailable.into()
        })
    }

    fn start(data_dir: &Path, run_id: &str) -> std::io::Result<Self> {
        Self::start_with_cleanup(data_dir, run_id, platform::remove_web_session)
    }

    fn start_with_cleanup(
        data_dir: &Path,
        run_id: &str,
        mut remove: impl FnMut(&Path) -> std::io::Result<()>,
    ) -> std::io::Result<Self> {
        let root = data_dir.join("web-sessions");
        std::fs::create_dir_all(&root)?;
        match abandoned_directories(&root) {
            Ok(paths) => {
                for path in paths {
                    if let Err(error) = remove(&path) {
                        tracing::warn!(error_kind = ?error.kind(), "abandoned original-page directory retained for next startup");
                    }
                }
            }
            Err(error) => {
                tracing::warn!(error_kind = ?error.kind(), "could not list abandoned original-page directories");
            }
        }
        let directory = fresh_directory(&root, run_id)?;
        Ok(Self {
            root,
            directory,
            serial: 0,
            route: None,
        })
    }

    pub fn next_label(&mut self, route: &WebviewRoute) -> Result<String, UiError> {
        if self.route.as_ref().is_some_and(|old| old != route) {
            return Err(WebError::ProxyRouteChanged.into());
        }
        self.serial += 1;
        Ok(format!("web-{}", self.serial))
    }

    pub fn opened(&mut self, route: WebviewRoute) {
        self.route = Some(route);
    }

    /// Called only after the host has observed destruction of every web window.
    pub fn clear(&mut self, run_id: &str) -> std::io::Result<()> {
        let next = fresh_directory(&self.root, run_id)?;
        let old = std::mem::replace(&mut self.directory, next);
        self.route = None;
        platform::remove_web_session(&old)
    }

    pub fn cleanup(&self) -> std::io::Result<()> {
        platform::remove_web_session(&self.directory)
    }
}

fn fresh_directory(root: &Path, id: &str) -> std::io::Result<PathBuf> {
    if id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    let path = root.join(id);
    std::fs::create_dir(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests;
