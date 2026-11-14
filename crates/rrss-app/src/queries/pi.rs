use std::path::Path;

use rusqlite::Connection;

use rrss_core::ai_store;
use rrss_core::db;
use rrss_core::interests;
use rrss_core::pi_store;

use crate::views::*;

/// 只读配置，不探测二进制、不启动 Pi；即使模型未配置也能编辑和修复。
pub fn pi_config(conn: &Connection) -> rrss_core::Result<PiConfigView> {
    Ok(PiConfigView {
        settings_override: pi_store::pi_override(conn, pi_store::PI_SETTINGS_OVERRIDE_KEY)?
            .to_string(),
        models_override: pi_store::pi_override(conn, pi_store::PI_MODELS_OVERRIDE_KEY)?.to_string(),
        use_own_config: pi_store::use_own_config(conn)?,
        own_model: pi_store::own_model(conn)?.unwrap_or_default(),
    })
}

/// 页面首屏：只读库和文件系统，不执行 `pi`。版本与 adapter 由 [`pi_probe`] 另行返回。
pub fn pi_page(conn: &Connection, data_dir: &Path) -> PiPage {
    let explicit = db::get_setting(conn, ai_store::PI_BINARY_KEY)
        .ok()
        .flatten();
    let (source, bin) = crate::pi::launch::locate_pi(explicit.as_deref());
    let managed = pi_store::pi_managed_version(conn).ok().flatten();
    let manage_notice = managed
        .filter(|v| !v.is_empty())
        .map(|v| crate::ui_error::pi::PiUiError::managed(&v))
        .unwrap_or_else(|| {
            let kind = if crate::pi::install::uses_custom_pi_binary(conn, data_dir) {
                crate::ui_error::pi::PiUiError::CustomPath
            } else if crate::pi::install::managed_executable(data_dir).is_file() {
                crate::ui_error::pi::PiUiError::ManagedInstalled
            } else {
                crate::ui_error::pi::PiUiError::ManagedMissing
            };
            kind.failure("pi management status")
        });
    let tools = pi_store::list_tools(conn)
        .unwrap_or_default()
        .into_iter()
        .map(|t| PiToolView {
            name: t.name,
            label: t.label,
            description: t.description,
            default_description: t.default_description,
            enabled: t.enabled,
            confirm: t.confirm_required,
        })
        .collect();
    let enabled = pi_store::enabled_skill_names(conn).unwrap_or_default();
    let skills = crate::pi::launch::list_skill_dirs(data_dir)
        .into_iter()
        .map(|(name, user, path)| {
            let md = std::fs::read_to_string(path.join("SKILL.md")).unwrap_or_default();
            SkillView {
                name: name.clone(),
                description: crate::pi::launch::skill_description_from_md(&md),
                bundled: !user,
                enabled: enabled.iter().any(|n| n == &name),
                loaded: false,
            }
        })
        .collect();
    let mcp_servers = pi_store::list_mcp_servers(conn)
        .unwrap_or_default()
        .into_iter()
        .map(|s| {
            let logged_in = !pi_store::use_own_config(conn).unwrap_or(false)
                && crate::mcp::logged_in(data_dir, &s);
            let mut view = crate::mcp::view(s);
            view.logged_in = logged_in;
            view
        })
        .collect();
    PiPage {
        developer_tools: db::get_setting(conn, ai_store::DEVELOPER_TOOLS_KEY)
            .ok()
            .flatten()
            .as_deref()
            == Some("1"),
        fetch_url: db::get_setting(conn, ai_store::FETCH_URL_ENABLED_KEY)
            .ok()
            .flatten()
            .as_deref()
            == Some("1"),
        pi_path: explicit.clone().unwrap_or_default(),
        binary_source: bin
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        binary_source_code: Some(source),
        session_dir: data_dir.join("pi/sessions").display().to_string(),
        manage_notice: Some(manage_notice),
        system_prompt: crate::pi::workspace::SYSTEM_TEMPLATE.to_string(),
        thinking_levels: pi_store::cached_thinking_levels(conn).unwrap_or_default(),
        tools,
        skills,
        mcp_servers,
    }
}

/// 执行 `pi --version`，可能耗时数百毫秒，调用方须放在阻塞线程池。MCP adapter 只看托管目录里的包声明。
/// 版本闸与配置检查复用启动流程的判断，不启动进程。
pub fn pi_probe(conn: &Connection, data_dir: &Path) -> PiProbe {
    use crate::pi::compat::{VersionGate, compat};
    let explicit = db::get_setting(conn, ai_store::PI_BINARY_KEY)
        .ok()
        .flatten();
    let (binary_source, version, bin) = crate::pi::launch::probe_pi_version(explicit.as_deref());
    let version_state = if bin.is_empty() {
        "missing"
    } else {
        match compat().gate(crate::pi::adapter::PiVersion::parse(&version).as_ref()) {
            VersionGate::Supported => "ok",
            VersionGate::Unverified => "unverified",
            VersionGate::TooOld => "too_old",
        }
    };
    let config_error = match crate::pi::launch::current_profile(conn, data_dir, None) {
        Err(err) if err.kind == crate::pi::ChatErrorKind::Config => Some(err.message),
        _ => None,
    };
    PiProbe {
        binary_source: bin,
        binary_source_code: Some(binary_source),
        version,
        version_state: version_state.into(),
        config_error,
    }
}

/// 编辑弹层用的用户技能原文；随应用提供的技能不可编辑。
pub fn pi_skill(data_dir: &Path, name: &str) -> anyhow::Result<SkillDraft> {
    crate::pi::launch::validate_skill_name(name)?;
    let path = crate::pi::workspace::WorkspacePaths::new(data_dir)
        .user_skill(name)
        .join("SKILL.md");
    let raw = std::fs::read_to_string(&path).map_err(|_| {
        anyhow::Error::new(crate::ui_error::pi::PiUiError::skill_draft_missing(name))
    })?;
    let body = raw
        .strip_prefix("---")
        .and_then(|rest| {
            rest.find("\n---")
                .map(|end| rest[end + 4..].trim().to_string())
        })
        .unwrap_or_else(|| raw.trim().to_string());
    Ok(SkillDraft {
        name: name.into(),
        description: crate::pi::launch::skill_description_from_md(&raw),
        body,
    })
}

pub fn interest_hits(conn: &Connection, id: i64) -> Vec<InterestHitView> {
    interests::list_interest_hits(conn, id)
        .unwrap_or_default()
        .into_iter()
        .map(|h| InterestHitView {
            id: h.article_id,
            title: h.title,
            feed_title: h.feed_title,
            weight: h.weight,
        })
        .collect()
}
