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
pub(crate) use picker::{picker_viewport_start, visible_picker_indices};
pub(crate) use state::{
    AddTaskMode, AddTaskState, CommandAvailabilityOverride, CommandState, ConfirmIntent,
    HeaderMenuAction, HeaderMenuItem, HeaderMenuKind, HeaderMenuState, MultilineInputMode,
    MultilineInputState, MultilineIntent, OrderMenuState, OverlayOutcome, OverlayState, OverlaySubmit,
    OverlayTarget, PickerIntent, PickerItem, PickerMode, PickerState, RECURRENCE_HISTORY_PAGE_SIZE,
    RecurrenceHistoryAction, RecurrenceHistoryEntryKey, RecurrenceHistoryState,
    ScheduleEditorField, ScheduleEditorMode, ScheduleEditorState, SearchIntent, SearchResultItem,
    SearchState, TagComboboxIntent, TextIntent, TextPanelState, UpdateOverlayState,
};
#[cfg(test)]
pub(crate) use state::{ConfirmState, TextInputState};
pub(crate) use tag_combobox::{tag_combobox_completion, tag_combobox_matches};
pub(crate) use text_input::LineEdit;
#[cfg(test)]
pub(crate) use view::{AddTaskAttachmentsView, TagComboboxKind};
pub(crate) use view::{
    AddTaskView, ConfirmView, HeaderMenuView, MultilineInputKind, MultilineInputView,
    OrderMenuView, OverlayView, PickerKind, PickerView, RecurrenceHistoryView, SearchKind,
    TagComboboxView, TextInputKind, TextInputView, TextPanelView,
};
