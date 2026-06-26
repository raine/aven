#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToastSeverity {
    Info,
    Warning,
    Error,
    Success,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Toast {
    pub(crate) message: String,
    pub(crate) severity: ToastSeverity,
    pub(crate) icon: bool,
}

impl Toast {
    pub(crate) fn new(message: impl Into<String>, severity: ToastSeverity) -> Self {
        Self {
            message: message.into(),
            severity,
            icon: true,
        }
    }

    pub(crate) fn without_icon(mut self) -> Self {
        self.icon = false;
        self
    }
}
