#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = mini_dns_relay::Config::from_env();
    mini_dns_relay::run(config).await
}
