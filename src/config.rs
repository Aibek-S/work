//! Optional TOML config at `$XDG_CONFIG_HOME/work/config.toml`
//! (falling back to `~/.config/work/config.toml`). The file is only read, never created.

use crate::noise::NoiseKind;
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const DEFAULT_DURATION_MIN: u32 = 30;
pub const DEFAULT_VOLUME: u8 = 35;
pub const MAX_DURATION_MIN: u32 = 24 * 60;

/// Raw file contents. Every field is optional; unknown keys are rejected so typos surface.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    pub duration: Option<u32>,
    pub noise: Option<String>,
    pub volume: Option<u8>,
    pub notifications: Option<bool>,
    pub theme: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    /// Calm colour palette (default).
    Minimal,
    /// No colours at all, only bold/dim attributes.
    Mono,
}

impl Theme {
    pub fn parse(s: &str) -> Option<Theme> {
        match s.trim().to_ascii_lowercase().as_str() {
            "minimal" | "default" => Some(Theme::Minimal),
            "mono" | "none" | "plain" => Some(Theme::Mono),
            _ => None,
        }
    }
}

/// Fully resolved settings used by the app.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub duration_min: u32,
    pub noise: NoiseKind,
    pub volume: u8,
    pub silent: bool,
    pub notifications: bool,
    pub tui: bool,
    pub theme: Theme,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            duration_min: DEFAULT_DURATION_MIN,
            noise: NoiseKind::White,
            volume: DEFAULT_VOLUME,
            silent: false,
            notifications: true,
            tui: true,
            theme: Theme::Minimal,
        }
    }
}

pub fn config_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if Path::new(&dir).is_absolute() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(base.join("work").join("config.toml"))
}

/// Loads the config file if it exists. A missing file is not an error.
pub fn load(path: &Path) -> Result<Option<FileConfig>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    parse(&text)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub fn parse(text: &str) -> Result<FileConfig, String> {
    toml::from_str(text).map_err(|e| e.message().to_string())
}

pub fn validate_duration(min: u32) -> Result<u32, String> {
    if (1..=MAX_DURATION_MIN).contains(&min) {
        Ok(min)
    } else {
        Err(format!(
            "duration must be between 1 and {MAX_DURATION_MIN} minutes, got {min}"
        ))
    }
}

pub fn validate_volume(v: u32) -> Result<u8, String> {
    if v <= 100 {
        Ok(v as u8)
    } else {
        Err(format!("volume must be between 0 and 100, got {v}"))
    }
}

pub fn parse_noise(s: &str) -> Result<NoiseKind, String> {
    NoiseKind::parse(s)
        .ok_or_else(|| format!("unknown noise '{s}' (expected white, pink or brown)"))
}

impl Settings {
    /// Applies file values on top of the current settings.
    pub fn apply_file(&mut self, f: &FileConfig) -> Result<(), String> {
        if let Some(d) = f.duration {
            self.duration_min = validate_duration(d)?;
        }
        if let Some(n) = &f.noise {
            self.noise = parse_noise(n)?;
        }
        if let Some(v) = f.volume {
            self.volume = validate_volume(v.into())?;
        }
        if let Some(n) = f.notifications {
            self.notifications = n;
        }
        if let Some(t) = &f.theme {
            self.theme = Theme::parse(t)
                .ok_or_else(|| format!("unknown theme '{t}' (expected minimal or mono)"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_example() {
        let f = parse(
            "duration = 30\nnoise = \"white\"\nvolume = 35\nnotifications = true\ntheme = \"minimal\"\n",
        )
        .unwrap();
        let mut s = Settings::default();
        s.apply_file(&f).unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn partial_file_overrides_only_given_keys() {
        let f = parse("noise = \"brown\"\nvolume = 10").unwrap();
        let mut s = Settings::default();
        s.apply_file(&f).unwrap();
        assert_eq!(s.noise, NoiseKind::Brown);
        assert_eq!(s.volume, 10);
        assert_eq!(s.duration_min, DEFAULT_DURATION_MIN);
    }

    #[test]
    fn rejects_bad_values() {
        let mut s = Settings::default();
        assert!(parse("durration = 5").is_err());
        assert!(parse("volume = -3").is_err());
        assert!(s.apply_file(&parse("volume = 150").unwrap()).is_err());
        assert!(s.apply_file(&parse("duration = 0").unwrap()).is_err());
        assert!(s.apply_file(&parse("noise = \"blue\"").unwrap()).is_err());
        assert!(s.apply_file(&parse("theme = \"neon\"").unwrap()).is_err());
    }

    #[test]
    fn missing_file_is_fine() {
        assert!(
            load(Path::new("/nonexistent/work/config.toml"))
                .unwrap()
                .is_none()
        );
    }
}
