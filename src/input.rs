use std::fs;
use std::io::{self, Read};
use std::path::Path;

use anyhow::{Context, Result, bail};

pub(crate) fn read_optional_text(
    inline: Option<String>,
    file: Option<&Path>,
    stdin_flag: bool,
    name: &str,
) -> Result<Option<String>> {
    let count = inline.is_some() as u8 + file.is_some() as u8 + stdin_flag as u8;
    if count > 1 {
        bail!("error multiple-{name}-sources");
    }
    if let Some(text) = inline {
        Ok(Some(text))
    } else if let Some(path) = file {
        Ok(Some(fs::read_to_string(path).with_context(|| {
            format!("could not read {}", path.display())
        })?))
    } else if stdin_flag {
        let mut text = String::new();
        io::stdin().read_to_string(&mut text)?;
        Ok(Some(text))
    } else {
        Ok(None)
    }
}

pub(crate) fn read_required_text(
    inline: Option<String>,
    file: Option<&Path>,
    stdin_flag: bool,
    name: &str,
) -> Result<String> {
    read_optional_text(inline, file, stdin_flag, name)?
        .with_context(|| format!("error missing-{name}"))
}
