use clap::ValueEnum;
use ratatui::style::{Color, Modifier, Style};

/// Rendering palettes are UI preferences and are intentionally not persisted
/// in projects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ThemeProfile {
    #[default]
    Dark,
    Light,
    HighContrast,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Theme {
    pub(super) profile: ThemeProfile,
}

impl Theme {
    pub(super) const fn new(profile: ThemeProfile) -> Self {
        Self { profile }
    }

    pub(super) fn accent(self, index: usize) -> Color {
        const DARK: [Color; 9] = [
            Color::Rgb(73, 207, 220),
            Color::Rgb(110, 214, 141),
            Color::Rgb(210, 126, 225),
            Color::Rgb(239, 194, 92),
            Color::Rgb(242, 116, 104),
            Color::Rgb(226, 139, 226),
            Color::Rgb(128, 156, 236),
            Color::Rgb(104, 190, 232),
            Color::Rgb(122, 217, 168),
        ];
        const LIGHT: [Color; 9] = [
            Color::Rgb(0, 112, 125),
            Color::Rgb(0, 119, 54),
            Color::Rgb(132, 35, 151),
            Color::Rgb(126, 81, 0),
            Color::Rgb(168, 36, 27),
            Color::Rgb(134, 35, 134),
            Color::Rgb(43, 76, 163),
            Color::Rgb(0, 91, 137),
            Color::Rgb(0, 105, 62),
        ];
        const HIGH_CONTRAST: [Color; 9] = [
            Color::Cyan,
            Color::Green,
            Color::Magenta,
            Color::Yellow,
            Color::Red,
            Color::LightMagenta,
            Color::Blue,
            Color::LightBlue,
            Color::LightGreen,
        ];

        match self.profile {
            ThemeProfile::Dark => DARK[index % DARK.len()],
            ThemeProfile::Light => LIGHT[index % LIGHT.len()],
            ThemeProfile::HighContrast => HIGH_CONTRAST[index % HIGH_CONTRAST.len()],
        }
    }

    pub(super) fn track_color(self, index: usize) -> Color {
        const DARK: [Color; 10] = [
            Color::Rgb(73, 207, 220),
            Color::Rgb(110, 214, 141),
            Color::Rgb(239, 194, 92),
            Color::Rgb(210, 126, 225),
            Color::Rgb(242, 116, 104),
            Color::Rgb(255, 153, 102),
            Color::Rgb(128, 156, 236),
            Color::Rgb(226, 139, 226),
            Color::Rgb(104, 190, 232),
            Color::Rgb(122, 217, 168),
        ];
        const LIGHT: [Color; 10] = [
            Color::Rgb(0, 112, 125),
            Color::Rgb(0, 119, 54),
            Color::Rgb(126, 81, 0),
            Color::Rgb(132, 35, 151),
            Color::Rgb(168, 36, 27),
            Color::Rgb(176, 74, 0),
            Color::Rgb(43, 76, 163),
            Color::Rgb(134, 35, 134),
            Color::Rgb(0, 91, 137),
            Color::Rgb(0, 105, 62),
        ];
        const HIGH_CONTRAST: [Color; 10] = [
            Color::Cyan,
            Color::Green,
            Color::Yellow,
            Color::Magenta,
            Color::Red,
            Color::LightRed,
            Color::Blue,
            Color::LightMagenta,
            Color::LightBlue,
            Color::LightGreen,
        ];

        match self.profile {
            ThemeProfile::Dark => DARK[index % DARK.len()],
            ThemeProfile::Light => LIGHT[index % LIGHT.len()],
            ThemeProfile::HighContrast => HIGH_CONTRAST[index % HIGH_CONTRAST.len()],
        }
    }

    pub(super) fn muted(self) -> Color {
        match self.profile {
            ThemeProfile::Dark => Color::Rgb(145, 155, 170),
            ThemeProfile::Light => Color::Rgb(92, 101, 112),
            ThemeProfile::HighContrast => Color::Gray,
        }
    }

    pub(super) fn disabled(self) -> Color {
        match self.profile {
            ThemeProfile::Dark => Color::Rgb(95, 105, 120),
            ThemeProfile::Light => Color::Rgb(135, 143, 151),
            ThemeProfile::HighContrast => Color::DarkGray,
        }
    }

    pub(super) fn occupied(self) -> Style {
        match self.profile {
            ThemeProfile::Dark => Style::default()
                .fg(Color::Rgb(235, 242, 250))
                .bg(Color::Rgb(42, 58, 78))
                .add_modifier(Modifier::BOLD),
            ThemeProfile::Light => Style::default()
                .fg(Color::Rgb(24, 35, 48))
                .bg(Color::Rgb(211, 224, 235))
                .add_modifier(Modifier::BOLD),
            ThemeProfile::HighContrast => Style::default()
                .fg(Color::White)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        }
    }

    pub(super) fn selected(self) -> Style {
        match self.profile {
            ThemeProfile::Dark => Style::default()
                .fg(Color::Rgb(7, 20, 29))
                .bg(Color::Rgb(72, 208, 220))
                .add_modifier(Modifier::BOLD),
            ThemeProfile::Light => Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(0, 112, 125))
                .add_modifier(Modifier::BOLD),
            ThemeProfile::HighContrast => Style::default()
                .fg(Color::Black)
                .bg(Color::White)
                .add_modifier(Modifier::BOLD),
        }
    }

    pub(super) fn playing(self) -> Style {
        match self.profile {
            ThemeProfile::Dark => Style::default()
                .fg(Color::Rgb(24, 22, 8))
                .bg(Color::Rgb(239, 194, 92))
                .add_modifier(Modifier::BOLD),
            ThemeProfile::Light => Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(160, 104, 0))
                .add_modifier(Modifier::BOLD),
            ThemeProfile::HighContrast => Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        }
    }

    pub(super) fn selected_playing(self) -> Style {
        match self.profile {
            ThemeProfile::Dark => Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(183, 86, 178))
                .add_modifier(Modifier::BOLD),
            ThemeProfile::Light => Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(133, 42, 126))
                .add_modifier(Modifier::BOLD),
            ThemeProfile::HighContrast => Style::default()
                .fg(Color::Black)
                .bg(Color::LightMagenta)
                .add_modifier(Modifier::BOLD),
        }
    }

    pub(super) fn selected_track(self) -> Style {
        self.selected()
    }

    pub(super) fn header(self) -> Style {
        self.selected()
    }

    pub(super) fn warning(self) -> Style {
        match self.profile {
            ThemeProfile::Dark => Style::default()
                .fg(Color::Rgb(28, 22, 4))
                .bg(Color::Rgb(245, 205, 93))
                .add_modifier(Modifier::BOLD),
            ThemeProfile::Light => Style::default()
                .fg(Color::Rgb(52, 35, 0))
                .bg(Color::Rgb(250, 211, 107))
                .add_modifier(Modifier::BOLD),
            ThemeProfile::HighContrast => Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        }
    }

    pub(super) fn recording(self) -> Style {
        match self.profile {
            ThemeProfile::Dark => Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(190, 48, 44))
                .add_modifier(Modifier::BOLD),
            ThemeProfile::Light => Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(177, 31, 28))
                .add_modifier(Modifier::BOLD),
            ThemeProfile::HighContrast => Style::default()
                .fg(Color::White)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD),
        }
    }

    pub(super) fn lock(self) -> Color {
        self.accent(5)
    }
}
