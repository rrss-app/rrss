use super::UiErrorKind;
use rrss_core::Error;

error_codes!(FolderError {
    FolderNotFound => "folder_not_found",
    FolderNameInvalid => "folder_name_invalid",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    Some(UiErrorKind::Folders(match error {
        Error::FolderNotFound => FolderError::FolderNotFound,
        Error::InvalidFolderName => FolderError::FolderNameInvalid,
        _ => return None,
    }))
}
