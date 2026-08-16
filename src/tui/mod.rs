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
const DIRECT_PERCENTAGE_HINT: &str = "[`/-/1–9/0] 0/10–90/100%";

#[derive(Default)]
struct TerminalGuard {
    raw: bool,
    alternate_screen: bool,
    cursor_hidden: bool,
}
impl TerminalGuard {
    fn enter() -> Result<Self> {
        let mut guard = Self {
            raw: true,
            ..Self::default()
        };
        enable_raw_mode()?;
        guard.alternate_screen = true;
        execute!(stdout(), EnterAlternateScreen)?;
        guard.cursor_hidden = true;
        execute!(stdout(), crossterm::cursor::Hide)?;
        Ok(guard)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.cursor_hidden {
            let _ = execute!(stdout(), crossterm::cursor::Show);
        }
        if self.alternate_screen {
            let _ = execute!(stdout(), LeaveAlternateScreen);
        }
        if self.raw {
            let _ = disable_raw_mode();
        }
    }
}

mod controller;
mod controls;
mod input;
mod overlays;
mod render;
mod state;
mod theme;

pub use state::App;
pub use theme::ThemeProfile;

pub fn project_with_default_presets() -> Project {
    controller::project_with_default_presets().0
}

pub fn run(
    project: Project,
    path: Option<PathBuf>,
    audio: &mut Audio,
    theme: ThemeProfile,
) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), crossterm::cursor::Show, LeaveAlternateScreen);
        old_hook(info)
    }));
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut app = App::new_with_theme(project, path, theme);
    let mut redraw = true;
    while !app.quit {
        audio.reap_retired();
        let status_changed = controller::refresh_audio_status(&mut app, audio);
        let animations_before = app.fader_animations.len();
        app.fader_animations
            .retain(|animation| !animation.is_complete(Instant::now()));
        let animating = !app.fader_animations.is_empty();
        redraw |= status_changed || animations_before != app.fader_animations.len();
        if redraw || animating {
            terminal.draw(|f| render::draw(f, &app, audio))?;
            redraw = false;
        }
        if event::poll(Duration::from_millis(8))? {
            match event::read()? {
                Event::Key(k) => {
                    if k.kind == event::KeyEventKind::Press {
                        input::handle_key(&mut app, audio, k)?;
                        redraw = true;
                    }
                }
                Event::Resize(_, _) => redraw = true,
                _ => {}
            }
        }
    }
    audio.shutdown_recording();
    Ok(())
}

#[cfg(test)]
mod tests;
