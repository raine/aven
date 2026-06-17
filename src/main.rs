use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    atm::run_cli().await
}
