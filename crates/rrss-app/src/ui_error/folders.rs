use super::UiErrorKind;
use rrss_core::{Error, tree_order::TreeOrderError};

error_codes!(FolderError {
    InvalidItem => "tree_item_invalid",
    EmptyMove => "tree_move_empty",
    DuplicateItems => "tree_move_duplicate",
    Cycle => "folder_move_cycle",
    WrongParent => "tree_position_wrong_parent",
    TooDeep => "folder_depth_exceeded",
    FolderNotFound => "folder_not_found",
    FolderNameInvalid => "folder_name_invalid",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    Some(UiErrorKind::Folders(match error {
        Error::FolderNotFound => FolderError::FolderNotFound,
        Error::InvalidFolderName => FolderError::FolderNameInvalid,
        Error::InvalidTreeOrder(reason) => match reason {
            TreeOrderError::InvalidItem => FolderError::InvalidItem,
            TreeOrderError::EmptyMove => FolderError::EmptyMove,
            TreeOrderError::DuplicateItems => FolderError::DuplicateItems,
            TreeOrderError::Cycle => FolderError::Cycle,
            TreeOrderError::WrongParent => FolderError::WrongParent,
            TreeOrderError::TooDeep => FolderError::TooDeep,
        },
        _ => return None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::UiError;
    use serde_json::json;

    #[test]
    fn tree_rejections_have_codes_and_no_text_parameters() {
        for (reason, code) in [
            (TreeOrderError::InvalidItem, "tree_item_invalid"),
            (TreeOrderError::EmptyMove, "tree_move_empty"),
            (TreeOrderError::DuplicateItems, "tree_move_duplicate"),
            (TreeOrderError::Cycle, "folder_move_cycle"),
            (TreeOrderError::WrongParent, "tree_position_wrong_parent"),
            (TreeOrderError::TooDeep, "folder_depth_exceeded"),
        ] {
            let wire =
                serde_json::to_value(UiError::from(Error::InvalidTreeOrder(reason))).unwrap();
            assert_eq!(wire["code"], code);
            assert_eq!(wire.get("params").cloned().unwrap_or(json!({})), json!({}));
        }
    }
}
