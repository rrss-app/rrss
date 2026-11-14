use super::network::NetworkMessage;
use super::{UiErrorKind, params};
use rrss_core::{Error, error::PiFailure};

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum PiDownloadStage {
    ReleasesFetch,
    ReleasesStatus,
    ReleasesParse,
    AssetFetch,
    AssetStatus,
    AssetRead,
    ChecksumsFetch,
    ChecksumsStatus,
    ChecksumsRead,
}
impl PiDownloadStage {
    fn diagnostic(self) -> &'static str {
        match self {
            Self::ReleasesFetch => "fetch pi releases",
            Self::ReleasesStatus => "github releases status",
            Self::ReleasesParse => "parse releases json",
            Self::AssetFetch => "download pi asset",
            Self::AssetStatus => "pi asset status",
            Self::AssetRead => "read pi asset",
            Self::ChecksumsFetch => "fetch SHA256SUMS",
            Self::ChecksumsStatus => "SHA256SUMS status",
            Self::ChecksumsRead => "read SHA256SUMS",
        }
    }
}

error_codes!(PiUiError {
    ChecksumMismatch { expected: String, actual: String } => "pi_checksum_mismatch",
    WorkspaceCreate { file: String } => "pi_workspace_create",
    WorkspaceBounds { file: String } => "pi_workspace_bounds",
    WorkspaceWrite { file: String } => "pi_workspace_write",
    WorkspaceReplace { file: String } => "pi_workspace_replace",
    Ready => "pi_ready",
    UpToDate { version: String } => "pi_up_to_date",
    Downloaded { version: String } => "pi_downloaded",
    Updated { from: String, to: String } => "pi_updated",
    CustomUnmanaged => "pi_custom_unmanaged",
    MissingDisabled => "pi_missing_disabled",
    ManagedVersion { version: String } => "pi_managed_version",
    CustomPath => "pi_custom_path",
    ManagedInstalled => "pi_managed_installed",
    ManagedMissing => "pi_managed_missing",

    DownloadFailed { stage: PiDownloadStage, status: u16, message: NetworkMessage } => "pi_download_failed",
    StagingCleanup => "pi_staging_cleanup",
    StagingCreate => "pi_staging_create",
    VersionBackup => "pi_version_backup",
    VersionPromote => "pi_version_promote",
    ConfigWriteLock => "pi_config_write_lock",
    SkillDraftMissing { name: String } => "pi_skill_draft_missing",
    NotInstalled => "pi_not_installed",
    ChatModelUnset => "pi_chat_model_unset",
    ProviderUrlMissing { name: String } => "pi_provider_url_missing",
    VersionTooOld { found: String, min: String } => "pi_version_too_old",
    SkillNameLength => "pi_skill_name_length",
    SkillNameCharacters => "pi_skill_name_characters",
    SkillDescriptionMissing => "pi_skill_description_missing",
    SkillDescriptionLength => "pi_skill_description_length",
    SkillNotFound { name: String } => "pi_skill_not_found",
    ReleaseMissing { line: String } => "pi_release_missing",
    PackageUnsupported { file: String } => "pi_package_unsupported",
    PackageBinaryMissing { file: String } => "pi_package_binary_missing",
    PackageNotStandalone { file: String } => "pi_package_not_standalone",
    ChecksumMissing { file: String } => "pi_checksum_missing",
    PlatformUnsupported => "pi_platform_unsupported",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    let Error::Pi(reason) = error else {
        return None;
    };
    Some(UiErrorKind::Pi(match reason {
        PiFailure::NotInstalled => PiUiError::NotInstalled,
        PiFailure::ChatModelUnset => PiUiError::ChatModelUnset,
        PiFailure::ProviderUrlMissing { name } => PiUiError::ProviderUrlMissing {
            name: params::name(name),
        },
        PiFailure::VersionTooOld { found, min } => PiUiError::VersionTooOld {
            found: params::name(found),
            min: params::name(min),
        },
        PiFailure::SkillNameLength => PiUiError::SkillNameLength,
        PiFailure::SkillNameCharacters => PiUiError::SkillNameCharacters,
        PiFailure::SkillDescriptionMissing => PiUiError::SkillDescriptionMissing,
        PiFailure::SkillDescriptionLength => PiUiError::SkillDescriptionLength,
        PiFailure::SkillNotFound { name } => PiUiError::SkillNotFound {
            name: params::name(name),
        },
        PiFailure::ReleaseMissing { line } => PiUiError::ReleaseMissing {
            line: params::name(line),
        },
        PiFailure::PackageUnsupported { file } => PiUiError::PackageUnsupported {
            file: params::file_name(std::path::Path::new(file)).unwrap_or_default(),
        },
        PiFailure::PackageBinaryMissing { file } => PiUiError::PackageBinaryMissing {
            file: params::file_name(std::path::Path::new(file)).unwrap_or_default(),
        },
        PiFailure::PackageNotStandalone { file } => PiUiError::PackageNotStandalone {
            file: params::file_name(std::path::Path::new(file)).unwrap_or_default(),
        },
        PiFailure::ChecksumMissing { file } => PiUiError::ChecksumMissing {
            file: params::file_name(std::path::Path::new(file)).unwrap_or_default(),
        },
        PiFailure::PlatformUnsupported => PiUiError::PlatformUnsupported,
        _ => return None,
    }))
}

impl PiUiError {
    pub(crate) fn checksum(expected: &str, actual: &str) -> super::UiError {
        Self::ChecksumMismatch {
            expected: params::name(expected),
            actual: params::name(actual),
        }
        .failure("pi checksum mismatch")
    }
    pub(crate) fn workspace(stage: &str, path: &std::path::Path) -> super::UiError {
        let file = params::file_name(path).unwrap_or_default();
        let kind = match stage {
            "create" => Self::WorkspaceCreate { file },
            "bounds" => Self::WorkspaceBounds { file },
            "write" => Self::WorkspaceWrite { file },
            _ => Self::WorkspaceReplace { file },
        };
        kind.failure("pi workspace operation failed")
    }
    pub(crate) fn managed(version: &str) -> super::UiError {
        Self::ManagedVersion {
            version: params::name(version),
        }
        .failure("managed pi version")
    }

    pub fn failure(self, detail: impl AsRef<str>) -> super::UiError {
        super::UiError::new(UiErrorKind::Pi(self), detail)
    }
    pub fn skill_draft_missing(name: &str) -> super::UiError {
        Self::SkillDraftMissing {
            name: params::name(name),
        }
        .failure("user skill draft is unavailable")
    }
    pub fn download(stage: PiDownloadStage, error: Error) -> super::UiError {
        if let Error::Network {
            status, message, ..
        } = error
        {
            let message = match message {
                rrss_core::network::RequestErrorMessage::Timeout => NetworkMessage::Timeout,
                rrss_core::network::RequestErrorMessage::Connect => NetworkMessage::Connect,
                rrss_core::network::RequestErrorMessage::Redirect => NetworkMessage::Redirect,
                rrss_core::network::RequestErrorMessage::Decode => NetworkMessage::Decode,
                rrss_core::network::RequestErrorMessage::Other => NetworkMessage::Other,
            };
            Self::DownloadFailed {
                stage,
                status,
                message,
            }
            .failure(format!("{}: HTTP {status}", stage.diagnostic()))
        } else {
            error.into()
        }
    }
    pub fn project(error: anyhow::Error) -> super::UiError {
        Self::project_ref(&error)
    }
    pub fn project_ref(error: &anyhow::Error) -> super::UiError {
        if let Some(ui) = error.downcast_ref::<super::UiError>() {
            return ui.clone();
        }
        if let Some(core) = error.downcast_ref::<Error>() {
            let mut ui = super::UiError::from_core(core);
            if ui.code() == "legacy_text" {
                ui.detail = rrss_core::logsafe::scrub_secrets(&format!("{error:#}"));
            }
            return ui;
        }
        // Archive parsing, OS IO and process joins had only technical English diagnostics.
        super::UiError::new(
            UiErrorKind::Common(super::common::CommonError::Internal),
            crate::log::error(error.as_ref()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn install_and_skill_failures_have_safe_codes() {
        for (reason, code) in [
            (PiFailure::ChatModelUnset, "pi_chat_model_unset"),
            (PiFailure::SkillNameLength, "pi_skill_name_length"),
            (PiFailure::SkillNameCharacters, "pi_skill_name_characters"),
            (
                PiFailure::SkillDescriptionMissing,
                "pi_skill_description_missing",
            ),
            (
                PiFailure::SkillDescriptionLength,
                "pi_skill_description_length",
            ),
            (PiFailure::PlatformUnsupported, "pi_platform_unsupported"),
        ] {
            assert_eq!(super::super::UiError::from(Error::Pi(reason)).code(), code);
        }
        for (kind, code) in [
            (PiUiError::StagingCleanup, "pi_staging_cleanup"),
            (PiUiError::StagingCreate, "pi_staging_create"),
            (PiUiError::VersionBackup, "pi_version_backup"),
            (PiUiError::VersionPromote, "pi_version_promote"),
            (PiUiError::ConfigWriteLock, "pi_config_write_lock"),
        ] {
            assert_eq!(kind.failure("fixture IO error").code(), code);
        }
        let ui = PiUiError::skill_draft_missing("fixture");
        assert_eq!(ui.code(), "pi_skill_draft_missing");
        let ui = PiUiError::download(
            PiDownloadStage::AssetStatus,
            Error::Network {
                kind: rrss_core::network::ErrorKind::Http,
                status: 503,
                message: rrss_core::network::RequestErrorMessage::Other,
            },
        );
        let wire = serde_json::to_value(ui).unwrap();
        assert_eq!(wire["code"], "pi_download_failed");
        assert_eq!(wire["params"]["status"], 503);
        assert_eq!(wire["params"]["stage"], "asset_status");
    }
}
