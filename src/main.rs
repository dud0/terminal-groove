use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use terminal_groove::{audio, persistence, tui};

#[derive(Parser, Debug)]
#[command(
    name = "terminal-groove",
    version,
    about = "A real-time terminal groovebox"
)]
struct Cli {
    #[arg(value_name = "PROJECT")]
    project: Option<PathBuf>,
    #[arg(long, value_name = "EXACT-NAME")]
    audio_device: Option<String>,
    #[arg(long)]
    list_audio_devices: bool,
    #[arg(long, value_name = "FRAMES")]
    audio_buffer: Option<u32>,
    #[arg(long, value_enum, default_value_t = tui::ThemeProfile::Dark)]
    theme: tui::ThemeProfile,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.list_audio_devices {
        for name in audio::output_device_names()? {
            println!("{name}")
        }
        return Ok(());
    }
    let project = match cli.project.as_deref() {
        Some(path) => persistence::load(path)
            .with_context(|| format!("startup project validation failed for {}", path.display()))?,
        None => tui::project_with_default_presets(),
    };
    let mut audio = audio::open(cli.audio_device.as_deref(), &project, cli.audio_buffer)
        .with_context(|| {
            format!(
                "audio diagnostics are written to {} when the log is available",
                audio::default_audio_log_path().display()
            )
        })?;
    tui::run(project, cli.project, &mut audio, cli.theme)
}
