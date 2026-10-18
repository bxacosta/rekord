//! User settings and where they are remembered between runs.

use crate::audio::DeviceChoice;
use crate::dsp::Params;
use crate::dsp::denoise::UNLIMITED_DB;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const GAIN_RANGE_DB: (f32, f32) = (-12.0, 30.0);
pub const GAIN_STEP_DB: f32 = 2.0;
/// Noise reduction strengths: the most noise that may be removed, in dB.
pub const DENOISE_LEVELS_DB: [f32; 7] = [6.0, 12.0, 18.0, 24.0, 30.0, 40.0, UNLIMITED_DB];

/// Environment variable that points to an alternative config file.
const CONFIG_ENV: &str = "REKORD_CONFIG";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub input: DeviceChoice,
    pub output: DeviceChoice,
    pub gain_db: f32,
    pub enhance: bool,
    pub denoise: bool,
    pub denoise_db: f32,
    /// Where recordings go. `None` means the default folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recordings_dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            input: DeviceChoice::Default,
            output: DeviceChoice::Default,
            gain_db: 12.0,
            enhance: false,
            denoise: false,
            denoise_db: 18.0,
            recordings_dir: None,
        }
    }
}

impl Settings {
    pub fn params(&self) -> Params {
        Params { gain_db: self.gain_db, enhance: self.enhance, denoise: self.denoise, denoise_db: self.denoise_db }
    }

    pub fn change_gain(&mut self, steps: i32) {
        let (min, max) = GAIN_RANGE_DB;
        self.gain_db = (self.gain_db + steps as f32 * GAIN_STEP_DB).clamp(min, max);
    }

    /// Index of the current noise reduction strength in [`DENOISE_LEVELS_DB`].
    pub fn denoise_level(&self) -> usize {
        DENOISE_LEVELS_DB.iter().position(|&db| db >= self.denoise_db).unwrap_or(DENOISE_LEVELS_DB.len() - 1)
    }

    pub fn change_denoise_level(&mut self, steps: i32) {
        let index = (self.denoise_level() as i32 + steps).clamp(0, DENOISE_LEVELS_DB.len() as i32 - 1);
        self.denoise_db = DENOISE_LEVELS_DB[index as usize];
    }

    /// Keeps hand-edited values within what the app supports.
    fn sanitized(mut self) -> Self {
        let (min, max) = GAIN_RANGE_DB;
        self.gain_db = if self.gain_db.is_finite() { self.gain_db.clamp(min, max) } else { Self::default().gain_db };
        self.denoise_db = DENOISE_LEVELS_DB[self.denoise_level()];
        self
    }
}

/// The settings file, `%APPDATA%\rekord\config.toml` on Windows.
pub struct SettingsFile {
    path: Option<PathBuf>,
}

impl SettingsFile {
    pub fn locate() -> Self {
        let path = std::env::var_os(CONFIG_ENV)
            .map(PathBuf::from)
            .or_else(|| dirs::config_dir().map(|dir| dir.join("rekord").join("config.toml")));
        Self { path }
    }

    /// Loads saved settings. A missing file gives the defaults; a broken one gives the
    /// defaults plus a warning, so a typo never stops the app from starting.
    pub fn load(&self) -> (Settings, Option<String>) {
        let Some(path) = &self.path else { return (Settings::default(), None) };
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str::<Settings>(&text) {
                Ok(settings) => (settings.sanitized(), None),
                Err(e) => (
                    Settings::default(),
                    Some(format!("Ignoring invalid settings in {}: {}", path.display(), e.message())),
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
            Err(e) => (Settings::default(), Some(format!("Could not read {}: {e}", path.display()))),
        }
    }

    /// Saves atomically: the file is either the old or the new version, never half-written.
    pub fn save(&self, settings: &Settings) -> Result<()> {
        let path = self.path.as_ref().context("No folder available for settings")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = path.with_extension("toml.tmp");
        std::fs::write(&temp, toml::to_string_pretty(settings)?)?;
        std::fs::rename(&temp, path).with_context(|| format!("Could not save {}", path.display()))?;
        Ok(())
    }
}

/// `Music\Rekord`, or a sensible fallback when there is no music folder.
pub fn default_recordings_dir() -> PathBuf {
    dirs::audio_dir()
        .or_else(dirs::home_dir)
        .map(|dir| dir.join("Rekord"))
        .unwrap_or_else(|| PathBuf::from("recordings"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_and_denoise_stay_in_range() {
        let mut s = Settings::default();
        s.change_gain(100);
        assert_eq!(s.gain_db, GAIN_RANGE_DB.1);
        s.change_gain(-100);
        assert_eq!(s.gain_db, GAIN_RANGE_DB.0);
        s.change_denoise_level(-10);
        assert_eq!(s.denoise_db, DENOISE_LEVELS_DB[0]);
        s.change_denoise_level(10);
        assert_eq!(s.denoise_db, UNLIMITED_DB);
    }

    #[test]
    fn saves_and_loads_settings() {
        let dir = tempfile::tempdir().unwrap();
        let file = SettingsFile { path: Some(dir.path().join("sub").join("config.toml")) };
        let settings = Settings { enhance: true, gain_db: 4.0, output: DeviceChoice::Off, ..Settings::default() };
        file.save(&settings).unwrap();
        assert_eq!(file.load(), (settings, None));
    }

    #[test]
    fn broken_or_partial_files_fall_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let file = SettingsFile { path: Some(path.clone()) };

        std::fs::write(&path, "enhance = true\ngain_db = 99").unwrap();
        let (settings, warning) = file.load();
        assert!(settings.enhance && warning.is_none());
        assert_eq!(settings.gain_db, GAIN_RANGE_DB.1);

        std::fs::write(&path, "gain_db = \"loud\"").unwrap();
        let (settings, warning) = file.load();
        assert_eq!(settings, Settings::default());
        assert!(warning.is_some());
    }
}
