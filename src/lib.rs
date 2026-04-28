mod app;
mod actions;
mod config;
mod i18n;
mod model;
mod plugins;

pub fn run() -> anyhow::Result<()> {
    let config = config::Config::load()?;
    app::run(config)
}
