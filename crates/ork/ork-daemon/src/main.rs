mod bootstrap;
mod api;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    bootstrap::init()?;
    bootstrap::poc_managed_agent_spawn();

    let config = bootstrap::load_config()?;
    let api_server = bootstrap::init_api(&config);
    let api_handle = tokio::spawn(async move {
        if let Err(e) = api_server.serve().await {
            tracing::error!("API server error: {e}");
        }
    });

    tokio::signal::ctrl_c().await?;
    tracing::info!("orkd kapatılıyor...");

    if let Err(e) = api_handle.await {
        tracing::warn!("API server join error: {e}");
    }

    Ok(())
}
