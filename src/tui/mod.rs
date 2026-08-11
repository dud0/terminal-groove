use crate::audio::Audio;
use crate::model::Project;
use anyhow::Result;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io::stdout,
    path::PathBuf,
    time::{Duration, Instant},
};

const DIRECT_PARAMETER_RAMP: Duration = Duration::from_millis(30);
const DIRECT_PERCENTAGE_HINT: &str = "[`/1–9/0] 0/10–90/100%";

struct TerminalGuard;
impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen, crossterm::cursor::Hide)?;
        Ok(Self)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), crossterm::cursor::Show, LeaveAlternateScreen);
    }
}

mod controller;
mod controls;
mod input;
mod overlays;
mod render;
mod state;

pub use state::App;

pub fn run(project: Project, path: Option<PathBuf>, audio: &mut Audio) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), crossterm::cursor::Show, LeaveAlternateScreen);
        old_hook(info)
    }));
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut app = App::new(project, path);
    while !app.quit {
        controller::refresh_audio_status(&mut app, audio);
        app.fader_animations
            .retain(|animation| !animation.is_complete(Instant::now()));
        terminal.draw(|f| render::draw(f, &app, audio))?;
        if event::poll(Duration::from_millis(8))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == event::KeyEventKind::Press {
                    input::handle_key(&mut app, audio, k)?
                }
            }
        }
    }
    audio.shutdown_recording();
    Ok(())
}

#[cfg(test)]
mod tests;
