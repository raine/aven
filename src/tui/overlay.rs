mod handlers;
mod layout;
pub(crate) mod metadata;
mod mouse;
mod multiline;
mod picker;
mod scroll;
mod state;
mod tag_combobox;
mod text_buffer;
mod text_input;
mod view;

pub(crate) use handlers::{
    handle_generic_overlay_key, handle_generic_overlay_mouse, handle_generic_overlay_paste,
    wrap_index_by_value,
};
#[cfg(test)]
pub(crate) use layout::COMMAND_DIALOG_MAX_WIDTH;
pub(crate) use layout::{
    CommandLayout, GENERIC_PICKER_VIEWPORT_ROWS, TAG_COMBOBOX_VIEWPORT_ROWS, TAG_COMBOBOX_WIDTH,
    command_layout, confirm_layout, confirm_width, dialog_area, dialog_inner_area, picker_layout,
    tag_combobox_layout, text_panel_layout, text_panel_scroll_cap,
};
pub(crate) use mouse::{OverlayMouseContext, OverlayMouseOutcome, dispatch_overlay_mouse};
pub(crate) use picker::{
    normalize_picker_selection, picker_viewport_start, sync_project_creation_item,
};
pub(crate) use state::{
    AddTaskMode, AddTaskState, ChangelogState, CommandAvailabilityOverride, CommandState,
    ConfirmIntent, EpicChildRemovalRestoration, HeaderMenuAction, HeaderMenuItem, HeaderMenuKind,
    HeaderMenuState, MultilineInputMode, MultilineInputState, MultilineIntent, OrderMenuState,
    OverlayOutcome, OverlayState, OverlaySubmit, OverlayTarget, PickerIntent, PickerItem,
    PickerMode, PickerState, RECURRENCE_HISTORY_PAGE_SIZE, RecurrenceHistoryAction,
    RecurrenceHistoryEntryKey, RecurrenceHistoryState, ScheduleEditorField, ScheduleEditorMode,
    ScheduleEditorState, SearchIntent, SearchResultItem, SearchState, SyncStatusAction,
    SyncStatusState, TagComboboxIntent, TextIntent, TextPanelState, UpdateActionFocus,
    UpdateNotesState, UpdateOverlayState, header_menu_area,
};
#[cfg(test)]
pub(crate) use state::{ConfirmState, TextInputState};

pub(crate) use text_buffer::TextBuffer;
pub(crate) use text_input::LineEdit;
#[cfg(test)]
pub(crate) use view::{AddTaskAttachmentsView, TagComboboxKind};
pub(crate) use view::{
    AddTaskView, ConfirmView, HeaderMenuView, MultilineInputKind, MultilineInputView,
    OrderMenuView, OverlayView, OverlayViewContext, PickerKind, PickerView, RecurrenceHistoryView,
    SearchKind, SyncStatusView, TagComboboxView, TextInputKind, TextInputView, TextPanelView,
};
