mod handlers;
mod layout;
mod multiline;
mod picker;
mod scroll;
mod state;
mod tag_combobox;
mod text_input;
mod view;

pub(crate) use handlers::{
    handle_generic_overlay_key, handle_generic_overlay_mouse, handle_generic_overlay_paste,
    wrap_index_by_value,
};
pub(crate) use layout::{
    GENERIC_PICKER_VIEWPORT_ROWS, GENERIC_PICKER_WIDTH, PROJECT_PICKER_VIEWPORT_ROWS,
    PROJECT_PICKER_WIDTH, TAG_COMBOBOX_VIEWPORT_ROWS, TAG_COMBOBOX_WIDTH, TEXT_PANEL_VISIBLE_ROWS,
    TEXT_PANEL_WIDTH, confirm_layout, confirm_width, dialog_area, picker_layout,
    tag_combobox_layout, text_panel_layout, text_panel_scroll_cap,
};
pub(crate) use picker::picker_viewport_start;
pub(crate) use state::{
    AddTaskState, CommandState, ConfirmSubmitRoute, HeaderMenuAction, HeaderMenuItem,
    HeaderMenuKind, HeaderMenuState, MultilineInputState, MultilineSubmitRoute, OrderMenuState,
    OverlayOutcome, OverlayRoute, OverlayState, OverlaySubmit, OverlaySubmitKind, PickerItem,
    PickerMode, PickerSubmitRoute, SearchPurpose, SearchResultItem, SearchState, TextPanelState,
    TextSubmitRoute,
};
#[cfg(test)]
pub(crate) use state::{ConfirmState, PickerState, TextInputState};
pub(crate) use text_input::LineEdit;
pub(crate) use view::{
    AddTaskView, ConfirmView, HeaderMenuView, MultilineInputView, OrderMenuView, OverlayView,
    PickerView, TagComboboxView, TextInputView, TextPanelView,
};
