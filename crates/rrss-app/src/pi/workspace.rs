//! 托管目录 `<data>/pi/`：路径约定、生成文件的原子写入、旧 `pi-project` 用户技能迁移。
//! 只重写 RRSS 生成的文件，不删除 Pi 自己写入的内容（包缓存、`crashes.json` 等）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// 随应用发布的扩展与技能，写进 `runtime/`。
pub const EXTENSION: &str = include_str!("../../../../pi/extensions/rrss-tools.ts");
pub const MCP_GUARD: &str = include_str!("../../../../pi/extensions/mcp-guard.ts");
const SKILL_CITE_VERIFY: &str = include_str!("../../../../pi/skills/cite-verify/SKILL.md");
pub const BUNDLED_SKILLS: &[(&str, &str)] = &[("cite-verify", SKILL_CITE_VERIFY)];

/// 固定系统提示模板（带版本注释），写成 `project/.pi/SYSTEM.md`，替换 Pi 默认的编程助手提示。
pub const SYSTEM_TEMPLATE: &str = include_str!("../../../../pi/SYSTEM.md");

/// 模板加上用户的助手说明。说明只追加在后面，不能替换模板。
pub fn system_prompt(note: &str) -> String {
    let mut out = SYSTEM_TEMPLATE.trim_end().to_string();
    let note = note.trim();
    if !note.is_empty() {
        out.push_str("\n\n## 用户补充说明\n\n以下是用户写给你的说明，与上面的规则冲突时以上面的规则为准。\n\n");
        out.push_str(note);
    }
    out.push('\n');
    out
}

/// 扩展与随应用技能的内容哈希，参与 Profile 指纹。
pub fn runtime_digest() -> String {
    let mut all = EXTENSION.to_string();
    all.push_str(MCP_GUARD);
    all.push_str(crate::mcp::POLICY);
    for (name, body) in BUNDLED_SKILLS {
        all.push_str(name);
        all.push_str(body);
    }
    super::profile::digest(all.as_bytes())
}

/// 写在 agent 目录里的标记，提示这里的生成文件每次启动都会重写。
pub const MANAGED_MARKER: &str = ".rrss-managed";
const MANAGED_NOTE: &str =
    "本目录由 RRSS 托管：settings.json、models.json 每次启动 Pi 时重写，请勿手改。\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspacePaths {
    pub root: PathBuf,
    /// `PI_CODING_AGENT_DIR`。
    pub agent: PathBuf,
    /// Pi 的 cwd。
    pub project: PathBuf,
    /// 扩展与随应用提供的技能，只用 `-e` / `--skill` 显式加载。
    pub runtime: PathBuf,
    /// 用户技能，唯一真源。
    pub user_skills: PathBuf,
    pub sessions: PathBuf,
}

impl WorkspacePaths {
    pub fn new(data_dir: &Path) -> Self {
        let root = data_dir.join("pi");
        Self {
            agent: root.join("agent"),
            project: root.join("project"),
            runtime: root.join("runtime"),
            user_skills: root.join("user-skills"),
            sessions: root.join("sessions"),
            root,
        }
    }

    pub fn extension(&self) -> PathBuf {
        self.runtime.join("rrss-tools.ts")
    }

    pub fn bundled_skill(&self, name: &str) -> PathBuf {
        self.runtime.join("skills").join(name)
    }

    pub fn user_skill(&self, name: &str) -> PathBuf {
        self.user_skills.join(name)
    }

    pub fn system_md(&self) -> PathBuf {
        self.project.join(".pi/SYSTEM.md")
    }

    pub fn tool_manifest(&self) -> PathBuf {
        self.project.join(".pi/tool-manifest.json")
    }

    /// 托管模式用 agent 配置；自有配置用 RRSS 项目配置并用 --approve 授权。
    pub fn mcp_config(&self, managed: bool) -> PathBuf {
        if managed {
            self.agent.join("mcp.json")
        } else {
            self.project.join(".pi/mcp.json")
        }
    }

    pub fn agent_settings(&self) -> PathBuf {
        self.agent.join("settings.json")
    }
}

/// 一份要写入托管目录的文件；`path` 相对 [`WorkspacePaths::root`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    pub path: PathBuf,
    pub contents: Vec<u8>,
}

impl GeneratedFile {
    pub fn new(path: impl Into<PathBuf>, contents: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            contents: contents.into(),
        }
    }
}

/// 先把全部文件写进暂存目录，成功后再逐个原子替换。任何一步失败都不留下半套配置。
pub fn write_files(paths: &WorkspacePaths, files: &[GeneratedFile]) -> Result<()> {
    // Login and a conversation may both regenerate agent/mcp.json.
    static WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _write = WRITES.lock().map_err(|_| {
        anyhow::Error::new(
            crate::ui_error::pi::PiUiError::ConfigWriteLock
                .failure("pi config write lock poisoned"),
        )
    })?;
    for dir in [
        &paths.agent,
        &paths.project,
        &paths.runtime,
        &paths.user_skills,
        &paths.sessions,
    ] {
        std::fs::create_dir_all(dir)
            .with_context(|| crate::ui_error::pi::PiUiError::workspace("create", dir))?;
    }
    let staging = paths.root.join(format!(".staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let result = (|| -> Result<()> {
        for (i, file) in files.iter().enumerate() {
            if file.path.is_absolute()
                || file
                    .path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                anyhow::bail!(crate::ui_error::pi::PiUiError::workspace(
                    "bounds", &file.path
                ));
            }
            let staged = staging.join(i.to_string());
            std::fs::create_dir_all(&staging)
                .with_context(|| crate::ui_error::pi::PiUiError::workspace("create", &staging))?;
            std::fs::write(&staged, &file.contents)
                .with_context(|| crate::ui_error::pi::PiUiError::workspace("write", &file.path))?;
        }
        for (i, file) in files.iter().enumerate() {
            let dest = paths.root.join(&file.path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| crate::ui_error::pi::PiUiError::workspace("create", parent))?;
            }
            if std::fs::read(&dest).ok().as_deref() == Some(file.contents.as_slice()) {
                continue;
            }
            std::fs::rename(staging.join(i.to_string()), &dest)
                .with_context(|| crate::ui_error::pi::PiUiError::workspace("replace", &dest))?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// 托管标记文件。
pub fn marker_file() -> GeneratedFile {
    GeneratedFile::new(Path::new("agent").join(MANAGED_MARKER), MANAGED_NOTE)
}

fn legacy_skills_root(data_dir: &Path) -> PathBuf {
    data_dir.join("pi-project/.pi/skills")
}

/// 把旧 `<data>/pi-project/.pi/skills/<name>` 的用户技能复制到 `user-skills/`，全部校验通过后删除旧 `pi-project/`。
/// 目标已有同名技能时保留目标。任何一步失败都保留旧目录，下次再试。
pub fn migrate_legacy_skills(data_dir: &Path) -> Result<()> {
    let legacy_project = data_dir.join("pi-project");
    if !legacy_project.try_exists()? {
        return Ok(());
    }
    let paths = WorkspacePaths::new(data_dir);
    std::fs::create_dir_all(&paths.user_skills)?;
    let entries = match std::fs::read_dir(legacy_skills_root(data_dir)) {
        Ok(entries) => Some(entries),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(err).context("read legacy skill directory; preserve legacy project");
        }
    };
    if let Some(entries) = entries {
        for entry in entries {
            let entry =
                entry.context("read legacy skill directory entry; preserve legacy project")?;
            let from = entry.path();
            let name = entry.file_name().into_string().map_err(|_| {
                anyhow::anyhow!("legacy skill name is not UTF-8; preserve legacy project")
            })?;
            if !entry.file_type()?.is_dir() || name.starts_with('_') {
                continue;
            }
            match std::fs::metadata(from.join("SKILL.md")) {
                Ok(meta) if meta.is_file() => {}
                Ok(_) => continue,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err).context("read legacy skill; preserve legacy project"),
            }
            let to = paths.user_skill(&name);
            if !to.join("SKILL.md").try_exists()? {
                // 完整复制成功才发布，失败的半成品不能在下次被误认作已有技能。
                let staged = paths.root.join(format!(".migrating-skill-{name}"));
                if staged.try_exists()? {
                    std::fs::remove_dir_all(&staged)?;
                }
                copy_dir(&from, &staged).with_context(|| format!("migrate skill {name}"))?;
                if to.try_exists()? {
                    anyhow::bail!(
                        "target skill directory {name} incomplete; preserve legacy project"
                    );
                }
                std::fs::rename(&staged, &to)?;
            }
            if !std::fs::metadata(to.join("SKILL.md"))?.is_file() {
                anyhow::bail!(
                    "skill {name} validation after migration failed; preserve legacy project"
                );
            }
        }
    }
    std::fs::remove_dir_all(&legacy_project)
        .with_context(|| format!("remove {}", legacy_project.display()))?;
    tracing::info!("migrated legacy pi-project skills to pi/user-skills");
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_generated_files_and_leaves_pi_owned_files_alone() {
        let dir = tempfile::tempdir().unwrap();
        let paths = WorkspacePaths::new(dir.path());
        std::fs::create_dir_all(paths.agent.join("npm/pkg")).unwrap();
        std::fs::write(paths.agent.join("npm/pkg/index.js"), "pi owns this").unwrap();
        std::fs::write(paths.agent.join("crashes.json"), "[]").unwrap();
        let files = vec![
            marker_file(),
            GeneratedFile::new("agent/settings.json", "{}"),
            GeneratedFile::new("project/.pi/settings.json", "{\"defaultTools\":[]}"),
        ];
        write_files(&paths, &files).unwrap();
        write_files(&paths, &files).unwrap();
        assert_eq!(
            std::fs::read_to_string(paths.agent_settings()).unwrap(),
            "{}"
        );
        assert!(paths.agent.join(MANAGED_MARKER).is_file());
        assert!(paths.project.join(".pi/settings.json").is_file());
        assert!(paths.sessions.is_dir() && paths.user_skills.is_dir());
        assert_eq!(
            std::fs::read_to_string(paths.agent.join("npm/pkg/index.js")).unwrap(),
            "pi owns this"
        );
        assert!(paths.agent.join("crashes.json").is_file());
        let leftovers: Vec<_> = std::fs::read_dir(&paths.root)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".staging"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn a_bad_path_aborts_before_anything_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let paths = WorkspacePaths::new(dir.path());
        write_files(&paths, &[GeneratedFile::new("agent/settings.json", "old")]).unwrap();
        let err = write_files(
            &paths,
            &[
                GeneratedFile::new("agent/settings.json", "new"),
                GeneratedFile::new("../escape.txt", "x"),
            ],
        )
        .unwrap_err();
        assert_eq!(crate::UiError::from(err).code(), "pi_workspace_bounds");
        assert_eq!(
            std::fs::read_to_string(paths.agent_settings()).unwrap(),
            "old"
        );
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[test]
    fn unreadable_legacy_skill_listing_preserves_project() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("pi-project/.pi");
        std::fs::create_dir_all(&legacy).unwrap();
        // ENOTDIR 同样是 read_dir 失败，且不依赖以 root 跑测试时的权限语义。
        std::fs::write(legacy.join("skills"), "must survive").unwrap();
        assert!(migrate_legacy_skills(dir.path()).is_err());
        assert_eq!(
            std::fs::read_to_string(legacy.join("skills")).unwrap(),
            "must survive"
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_copy_is_not_published_and_retry_copies_all_files() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("pi-project/.pi/skills/mine");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("SKILL.md"), "skill").unwrap();
        std::os::unix::fs::symlink("missing", legacy.join("notes.txt")).unwrap();
        assert!(migrate_legacy_skills(dir.path()).is_err());
        let target = WorkspacePaths::new(dir.path()).user_skill("mine");
        assert!(legacy.exists());
        assert!(
            !target.exists(),
            "partial copy must not become authoritative"
        );
        std::fs::remove_file(legacy.join("notes.txt")).unwrap();
        std::fs::write(legacy.join("notes.txt"), "notes").unwrap();
        migrate_legacy_skills(dir.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("notes.txt")).unwrap(),
            "notes"
        );
        assert!(!dir.path().join("pi-project").exists());
    }

    #[test]
    fn missing_legacy_skills_is_an_empty_migration() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pi-project/.pi")).unwrap();
        migrate_legacy_skills(dir.path()).unwrap();
        assert!(!dir.path().join("pi-project").exists());
    }

    #[test]
    fn legacy_user_skills_move_to_the_managed_dir() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("pi-project/.pi/skills");
        for (name, body) in [
            ("mine", "old mine"),
            ("kept", "old kept"),
            ("_bundled", "x"),
        ] {
            std::fs::create_dir_all(legacy.join(name)).unwrap();
            std::fs::write(legacy.join(name).join("SKILL.md"), body).unwrap();
        }
        std::fs::write(legacy.join("mine/notes.txt"), "extra").unwrap();
        let paths = WorkspacePaths::new(dir.path());
        std::fs::create_dir_all(paths.user_skill("kept")).unwrap();
        std::fs::write(paths.user_skill("kept").join("SKILL.md"), "new kept").unwrap();

        migrate_legacy_skills(dir.path()).unwrap();
        let read = |name: &str, file: &str| {
            std::fs::read_to_string(paths.user_skill(name).join(file)).unwrap()
        };
        assert_eq!(read("mine", "SKILL.md"), "old mine");
        assert_eq!(read("mine", "notes.txt"), "extra");
        assert_eq!(read("kept", "SKILL.md"), "new kept", "target wins");
        assert!(!paths.user_skill("_bundled").exists());
        assert!(!dir.path().join("pi-project").exists());
        // 再跑一次什么也不做。
        migrate_legacy_skills(dir.path()).unwrap();
    }
}
