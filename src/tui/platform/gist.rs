use anyhow::Result;

pub(crate) fn create_secret_gist(
    markdown: &str,
    filename: &str,
    description: &str,
) -> Result<String> {
    aven_host::gist::create_secret_gist(std::ffi::OsStr::new("gh"), markdown, filename, description)
}
