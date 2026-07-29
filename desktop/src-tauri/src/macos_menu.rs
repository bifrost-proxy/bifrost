use super::MACOS_APP_QUIT_MENU_ID;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MacosMenuAction {
    Quit,
    Edit(&'static str),
    Ignore,
}

pub(crate) fn macos_menu_action(menu_id: &str) -> MacosMenuAction {
    match menu_id {
        MACOS_APP_QUIT_MENU_ID => MacosMenuAction::Quit,
        "edit-undo" => MacosMenuAction::Edit("undo"),
        "edit-redo" => MacosMenuAction::Edit("redo"),
        "edit-select-all" => MacosMenuAction::Edit("editor.action.selectAll"),
        _ => MacosMenuAction::Ignore,
    }
}
