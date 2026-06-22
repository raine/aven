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
}

impl Toast {
    pub(crate) fn new(message: impl Into<String>, severity: ToastSeverity) -> Self {
        Self {
            message: message.into(),
            severity,
        }
    }
}
