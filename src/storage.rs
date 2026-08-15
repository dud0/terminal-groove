use anyhow::{Result, bail};
use std::path::PathBuf;

const APPLICATION_DIRECTORY_NAME: &str = "Terminal Groove";
const PROJECTS_DIRECTORY_NAME: &str = "Projects";
const PRESETS_DIRECTORY_NAME: &str = "Presets";
const RECORDINGS_DIRECTORY_NAME: &str = "Recordings";
const LOGS_DIRECTORY_NAME: &str = "Logs";

/// The visible root for files created by Terminal Groove.
///
/// The platform Music directory is preferred. Minimal environments that do
/// not expose one use a visible folder directly below the user's home.
pub(crate) fn root() -> Result<PathBuf> {
    root_from_user_directories(dirs::audio_dir(), dirs::home_dir())
}

pub(crate) fn projects_directory() -> Result<PathBuf> {
    Ok(root()?.join(PROJECTS_DIRECTORY_NAME))
}

pub(crate) fn presets_directory() -> Result<PathBuf> {
    Ok(root()?.join(PRESETS_DIRECTORY_NAME))
}

pub(crate) fn recordings_directory() -> Result<PathBuf> {
    Ok(root()?.join(RECORDINGS_DIRECTORY_NAME))
}

pub(crate) fn logs_directory() -> Result<PathBuf> {
    Ok(root()?.join(LOGS_DIRECTORY_NAME))
}

fn root_from_user_directories(music: Option<PathBuf>, home: Option<PathBuf>) -> Result<PathBuf> {
    match music.or(home) {
        Some(directory) => Ok(directory.join(APPLICATION_DIRECTORY_NAME)),
        None => bail!("could not determine the Music or home directory for Terminal Groove files"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn root_prefers_the_music_directory() {
        assert_eq!(
            root_from_user_directories(
                Some(PathBuf::from("/users/ada/Music")),
                Some(PathBuf::from("/users/ada")),
            )
            .unwrap(),
            Path::new("/users/ada/Music/Terminal Groove")
        );
    }

    #[test]
    fn root_falls_back_to_a_visible_home_folder() {
        assert_eq!(
            root_from_user_directories(None, Some(PathBuf::from("/users/ada"))).unwrap(),
            Path::new("/users/ada/Terminal Groove")
        );
    }

    #[test]
    fn root_rejects_missing_user_directories() {
        assert!(root_from_user_directories(None, None).is_err());
    }
}
