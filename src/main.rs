use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    if let Err(error) = aven::run_cli().await {
        tracing::error!(error = %error, "command failed");
        return Err(error);
    }
    Ok(())
}
