#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    ControlledClock,
    Barrier,
    LostReply,
    Restart,
}

pub fn require(available: &[Capability], required: &[Capability]) -> Result<(), Vec<Capability>> {
    let missing: Vec<_> = required
        .iter()
        .copied()
        .filter(|capability| !available.contains(capability))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_hooks_are_failures() {
        assert_eq!(
            require(
                &[Capability::Barrier],
                &[Capability::Barrier, Capability::Restart]
            ),
            Err(vec![Capability::Restart]),
        );
    }
}
