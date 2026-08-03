use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    nealsd::run().await
}
