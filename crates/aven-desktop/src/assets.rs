//! Assets compiled into the binary: icons, logo and the ref font.

use std::borrow::Cow;

use gpui::{App, AssetSource, Result, SharedString};

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: &'static [u8] = match path {
            "wordmark-dark.svg" => include_bytes!("../assets/wordmark-dark.svg"),
            "wordmark-light.svg" => include_bytes!("../assets/wordmark-light.svg"),
            "logo-dark.svg" => include_bytes!("../assets/logo-dark.svg"),
            "logo-light.svg" => include_bytes!("../assets/logo-light.svg"),
            "icons/cloud.svg" => include_bytes!("../assets/icons/cloud.svg"),
            "icons/device.svg" => include_bytes!("../assets/icons/device.svg"),
            "icons/server.svg" => include_bytes!("../assets/icons/server.svg"),
            "icons/ic-open.svg" => include_bytes!("../assets/icons/ic-open.svg"),
            "icons/ic-backlog.svg" => include_bytes!("../assets/icons/ic-backlog.svg"),
            "icons/ic-todo.svg" => include_bytes!("../assets/icons/ic-todo.svg"),
            "icons/ic-active.svg" => include_bytes!("../assets/icons/ic-active.svg"),
            "icons/ic-recent.svg" => include_bytes!("../assets/icons/ic-recent.svg"),
            "icons/ic-search.svg" => include_bytes!("../assets/icons/ic-search.svg"),
            "icons/ic-sparkle.svg" => include_bytes!("../assets/icons/ic-sparkle.svg"),
            "icons/ic-expand.svg" => include_bytes!("../assets/icons/ic-expand.svg"),
            "icons/chevron.svg" => include_bytes!("../assets/icons/chevron.svg"),
            "icons/ic-labels.svg" => include_bytes!("../assets/icons/ic-labels.svg"),
            "icons/ic-copy.svg" => include_bytes!("../assets/icons/ic-copy.svg"),
            "icons/ic-trash.svg" => include_bytes!("../assets/icons/ic-trash.svg"),
            "icons/ic-blocked.svg" => include_bytes!("../assets/icons/ic-blocked.svg"),
            "icons/ic-conflicts.svg" => include_bytes!("../assets/icons/ic-conflicts.svg"),
            "icons/ic-details.svg" => include_bytes!("../assets/icons/ic-details.svg"),
            "icons/ic-done.svg" => include_bytes!("../assets/icons/ic-done.svg"),
            "icons/epic-star.svg" => include_bytes!("../assets/icons/epic-star.svg"),
            "icons/ic-epics.svg" => include_bytes!("../assets/icons/ic-epics.svg"),
            "icons/ic-inbox.svg" => include_bytes!("../assets/icons/ic-inbox.svg"),
            "icons/ic-overdue.svg" => include_bytes!("../assets/icons/ic-overdue.svg"),
            "icons/ic-projects.svg" => include_bytes!("../assets/icons/ic-projects.svg"),
            "icons/ic-queue.svg" => include_bytes!("../assets/icons/ic-queue.svg"),
            "icons/ic-ready.svg" => include_bytes!("../assets/icons/ic-ready.svg"),
            "icons/ic-recurring.svg" => include_bytes!("../assets/icons/ic-recurring.svg"),
            "icons/ic-settings.svg" => include_bytes!("../assets/icons/ic-settings.svg"),
            "icons/ic-upcoming.svg" => include_bytes!("../assets/icons/ic-upcoming.svg"),
            "icons/nav-toggle.svg" => include_bytes!("../assets/icons/nav-toggle.svg"),
            "icons/ra-attach.svg" => include_bytes!("../assets/icons/ra-attach.svg"),
            "icons/ra-create.svg" => include_bytes!("../assets/icons/ra-create.svg"),
            "icons/ra-desc.svg" => include_bytes!("../assets/icons/ra-desc.svg"),
            "icons/ra-edit.svg" => include_bytes!("../assets/icons/ra-edit.svg"),
            "icons/ra-note.svg" => include_bytes!("../assets/icons/ra-note.svg"),
            "icons/pri-0.svg" => include_bytes!("../assets/icons/pri-0.svg"),
            "icons/pri-1-off.svg" => include_bytes!("../assets/icons/pri-1-off.svg"),
            "icons/pri-1-on.svg" => include_bytes!("../assets/icons/pri-1-on.svg"),
            "icons/pri-2-off.svg" => include_bytes!("../assets/icons/pri-2-off.svg"),
            "icons/pri-2-on.svg" => include_bytes!("../assets/icons/pri-2-on.svg"),
            "icons/pri-3-on.svg" => include_bytes!("../assets/icons/pri-3-on.svg"),
            "icons/pri-4.svg" => include_bytes!("../assets/icons/pri-4.svg"),
            "icons/st-active.svg" => include_bytes!("../assets/icons/st-active.svg"),
            "icons/st-canceled.svg" => include_bytes!("../assets/icons/st-canceled.svg"),
            "icons/st-done.svg" => include_bytes!("../assets/icons/st-done.svg"),
            "icons/st-inbox.svg" => include_bytes!("../assets/icons/st-inbox.svg"),
            "icons/st-ring.svg" => include_bytes!("../assets/icons/st-ring.svg"),
            "icons/toast-done.svg" => include_bytes!("../assets/icons/toast-done.svg"),
            "icons/toast-info.svg" => include_bytes!("../assets/icons/toast-info.svg"),
            "icons/toast-warn.svg" => include_bytes!("../assets/icons/toast-warn.svg"),
            "icons/toast-error.svg" => include_bytes!("../assets/icons/toast-error.svg"),
            "icons/toast-progress.svg" => include_bytes!("../assets/icons/toast-progress.svg"),
            "icons/toast-dismiss.svg" => include_bytes!("../assets/icons/toast-dismiss.svg"),
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(bytes)))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

pub fn load_fonts(cx: &App) {
    cx.text_system()
        .add_fonts(vec![
            Cow::Borrowed(include_bytes!("../assets/JetBrainsMono-Regular.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/JetBrainsMono-Medium.ttf").as_slice()),
        ])
        .expect("bundled fonts load");
}
