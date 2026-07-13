mod app;
mod asr;
mod audio;
mod commands;
mod config;
mod events;
mod hotkey;
mod logging;
mod output;
mod overlay;
mod paths;
mod platform;
mod smoke;
mod state;
mod tray;
mod ui;

use anyhow::Result;

fn main() -> Result<()> {
    platform::dpi::enable_per_monitor_awareness();
    let _log_guard = logging::initialize()?;
    std::panic::set_hook(Box::new(|panic| {
        tracing::error!(message = %panic, "unhandled panic");
    }));

    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|value| value == "--audio-smoke")
    {
        return smoke::run_audio();
    }
    if arguments
        .first()
        .is_some_and(|value| value == "--smoke-model")
    {
        if arguments.len() != 3 {
            anyhow::bail!("usage: dictate-in --smoke-model <model> <wav>");
        }
        let model = arguments[1].to_string_lossy();
        return smoke::run(&model, std::path::Path::new(&arguments[2]));
    }

    let _single_instance = platform::single_instance::SingleInstance::acquire()?;
    app::run()
}
