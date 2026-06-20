use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    aven::run_cli().await
}
