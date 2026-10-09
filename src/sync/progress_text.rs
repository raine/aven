use super::encrypted::Amount;
use crate::render::format_bytes;

/// Measured work in the current stage. A percentage appears only against an
/// exact total, and reaches 100% only when every byte has transferred.
pub(crate) fn amount_text(amount: Amount) -> String {
    match amount {
        Amount::Bytes {
            done,
            total: Some(total),
        } if total > 0 => format!(
            "{} of {} · {}%",
            format_bytes(done),
            format_bytes(total),
            u128::from(done.min(total)) * 100 / u128::from(total)
        ),
        Amount::Bytes { done, .. } => format!("{} so far", format_bytes(done)),
        Amount::Changes { done: 1 } => "1 change applied".to_string(),
        Amount::Changes { done } => format!("{done} changes applied"),
        Amount::Images { done, remaining } => format!("{done} done · {remaining} left"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_measurements_show_percentages_only_for_known_nonzero_totals() {
        for (done, total, expected) in [
            (1024, Some(4096), "1.0 KiB of 4.0 KiB · 25%"),
            (4095, Some(4096), "4.0 KiB of 4.0 KiB · 99%"),
            (4096, Some(4096), "4.0 KiB of 4.0 KiB · 100%"),
            (0, Some(0), "0 B so far"),
            (1024, None, "1.0 KiB so far"),
        ] {
            assert_eq!(amount_text(Amount::Bytes { done, total }), expected);
        }
    }
}
