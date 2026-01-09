pub mod app;
pub mod ui;

use anyhow::Result;

pub async fn run(lease: Option<String>) -> Result<()> {
    app::App::new(lease).run().await
}
