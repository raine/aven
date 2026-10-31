//! Every command the app exposes. Keys and menus both dispatch these actions, so
//! a command behaves the same however it is reached.
use gpui::{KeyBinding, Menu, MenuItem, OsAction, SystemMenuType, actions};

use crate::input;

actions!(
    aven,
    [
        Quit,
        CloseWindow,
        NewTask,
        ToggleSidebar,
        ToggleDetails,
        ToggleTheme,
        MoveDown,
        MoveUp,
        MarkDone,
        CancelTask,
        ChangeStatus,
        ChangePriority,
        EditTitle,
        CopyRef,
        Dismiss,
        CreateTask,
    ]
);

pub fn key_bindings() -> Vec<KeyBinding> {
    let main = Some("Aven && !TextInput && !Overlay");
    let dialog = Some("Dialog");
    vec![
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-n", NewTask, None),
        KeyBinding::new("a", NewTask, main),
        KeyBinding::new("j", MoveDown, main),
        KeyBinding::new("k", MoveUp, main),
        KeyBinding::new("down", MoveDown, main),
        KeyBinding::new("up", MoveUp, main),
        KeyBinding::new("d", MarkDone, main),
        KeyBinding::new("x", CancelTask, main),
        KeyBinding::new("s", ChangeStatus, main),
        KeyBinding::new("!", ChangePriority, main),
        KeyBinding::new("shift-1", ChangePriority, main),
        KeyBinding::new("e p", ChangePriority, main),
        KeyBinding::new("e t", EditTitle, main),
        KeyBinding::new("enter", EditTitle, main),
        KeyBinding::new("y r", CopyRef, main),
        KeyBinding::new("g b", ToggleSidebar, main),
        KeyBinding::new("g d", ToggleDetails, main),
        KeyBinding::new("cmd-enter", CreateTask, dialog),
        KeyBinding::new("ctrl-s", CreateTask, dialog),
        KeyBinding::new("escape", Dismiss, dialog),
    ]
}

pub fn menus() -> Vec<Menu> {
    vec![
        Menu::new("Aven").items([
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Aven", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Task", NewTask),
            MenuItem::separator(),
            MenuItem::action("Close Window", CloseWindow),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
            MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
            MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("View").items([
            MenuItem::action("Toggle Sidebar", ToggleSidebar),
            MenuItem::action("Toggle Details", ToggleDetails),
            MenuItem::action("Toggle Theme", ToggleTheme),
        ]),
        Menu::new("Task").items([
            MenuItem::action("Mark Done", MarkDone),
            MenuItem::action("Cancel Task", CancelTask),
            MenuItem::separator(),
            MenuItem::action("Status…", ChangeStatus),
            MenuItem::action("Priority…", ChangePriority),
            MenuItem::action("Edit Title", EditTitle),
            MenuItem::action("Copy Ref", CopyRef),
        ]),
    ]
}
