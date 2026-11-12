//! 本机数据目录。默认 `~/.local/share/rrss/` / `%APPDATA%\rrss\data\`。

use std::path::PathBuf;

use directories::ProjectDirs;

use crate::error::{Error, Result};

/// 应用数据根目录（SQLite、媒体缓存、日后 Pi 会话都放这里）。
///
/// 测试请用 [`crate::db::open_at`]，不要调用本函数去碰真实 home。
pub fn data_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "rrss").ok_or(Error::DataDirUnavailable)?;
    Ok(dirs.data_dir().to_path_buf())
}

/// 主库路径：`<data_dir>/rrss.db`。
pub fn db_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("rrss.db"))
}

/// 本机运行日志；Windows 使用 LocalAppData，不随数据目录漫游。
pub fn log_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "rrss").ok_or(Error::DataDirUnavailable)?;
    Ok(dirs.data_local_dir().join("logs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_under_rrss() {
        let dir = data_dir().expect("project dirs");
        #[cfg(unix)]
        {
            assert_eq!(
                dir.file_name().and_then(|n| n.to_str()),
                Some("rrss"),
                "unexpected data dir {dir:?}"
            );
        }
        #[cfg(windows)]
        {
            // `directories` puts data_dir at %APPDATA%\rrss\data on Windows.
            assert_eq!(
                dir.file_name().and_then(|n| n.to_str()),
                Some("data"),
                "unexpected data dir {dir:?}"
            );
            assert_eq!(
                dir.parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str()),
                Some("rrss"),
                "data dir should live under %APPDATA%\\rrss\\data, got {dir:?}"
            );
        }
    }

    #[test]
    fn db_path_is_rrss_db_under_data_dir() {
        let path = db_path().expect("db path");
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("rrss.db"));
        assert_eq!(path.parent().map(PathBuf::from), Some(data_dir().unwrap()));
    }
}
