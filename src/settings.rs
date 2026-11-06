use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language { #[default] System, Russian, English }

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme { #[default] Graphite, Classic, Amber, Ice, Ocean, Violet, Silver, Phosphor, AdwaitaLight, AdwaitaDark, Nord, Catppuccin, SolarizedLight }

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visualizer { #[default] Segments, Bars, Curve, Off, Mirror, Radial }

/// The named EQ preset is saved separately from the actual 10 gains.
/// Changing any slider switches to Custom, but does not lose the sound profile.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqPreset {
    #[default] Custom, Flat, Rock, BassBoost, Vocals,
    Pop, Electronic, Jazz, Classical, Acoustic, TrebleBoost,
}

impl EqPreset {
    pub fn from_index(index: u32) -> Self {
        match index {
            1 => Self::Flat, 2 => Self::Rock, 3 => Self::BassBoost,
            4 => Self::Vocals, 5 => Self::Pop, 6 => Self::Electronic,
            7 => Self::Jazz, 8 => Self::Classical, 9 => Self::Acoustic,
            10 => Self::TrebleBoost, _ => Self::Custom,
        }
    }

    pub fn index(self) -> u32 {
        match self {
            Self::Custom => 0, Self::Flat => 1, Self::Rock => 2,
            Self::BassBoost => 3, Self::Vocals => 4, Self::Pop => 5,
            Self::Electronic => 6, Self::Jazz => 7, Self::Classical => 8,
            Self::Acoustic => 9, Self::TrebleBoost => 10,
        }
    }

    pub fn gains(self) -> Option<[f64; 10]> {
        match self {
            Self::Custom => None,
            Self::Flat => Some([0.0; 10]),
            Self::Rock => Some([4.0, 3.0, 1.0, -1.0, -2.0, 0.0, 2.0, 3.0, 4.0, 3.0]),
            Self::BassBoost => Some([7.0, 6.0, 4.0, 2.0, 0.0, -1.0, -1.0, -1.0, -1.0, -1.0]),
            Self::Vocals => Some([-2.0, -2.0, -1.0, 1.0, 3.0, 4.0, 4.0, 2.0, 0.0, -1.0]),
            Self::Pop => Some([-1.0, 0.0, 2.0, 3.0, 2.0, 1.0, 2.0, 3.0, 2.0, 0.0]),
            Self::Electronic => Some([5.0, 4.0, 2.0, 0.0, -2.0, -1.0, 1.0, 3.0, 4.0, 4.0]),
            Self::Jazz => Some([2.0, 1.0, 0.0, 1.0, 2.0, 2.0, 1.0, 2.0, 2.0, 1.0]),
            Self::Classical => Some([1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 2.0]),
            Self::Acoustic => Some([-1.0, 0.0, 1.0, 2.0, 3.0, 2.0, 2.0, 1.0, 0.0, -1.0]),
            Self::TrebleBoost => Some([-2.0, -2.0, -1.0, -1.0, 0.0, 1.0, 3.0, 4.0, 5.0, 5.0]),
        }
    }

    /// Upgrade pre-0.9 configuration files, which stored gains but no preset name.
    pub fn infer_from_gains(gains: &[f64; 10]) -> Self {
        for n in 1..=10 {
            let candidate = Self::from_index(n);
            if let Some(target) = candidate.gains() {
                if target.iter().zip(gains).all(|(a, b)| (a - b).abs() < 0.01) {
                    return candidate;
                }
            }
        }
        Self::Custom
    }
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Repeat { #[default] Off, All, One }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub language: Language,
    pub theme: Theme,
    pub visualizer: Visualizer,
    pub music_root: Option<PathBuf>,
    pub autoplay_import: bool,
    pub remember_queue: bool,
    pub background_on_close: bool,
    pub volume: f64,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub eq_gains: [f64; 10],
    pub eq_preset: EqPreset,
    // GTK4 window geometry is recorded while the window is visible.
    // serde(default) keeps old v0.5 settings compatible.
    pub window_width: i32,
    pub window_height: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::System, theme: Theme::Graphite,
            visualizer: Visualizer::Segments, music_root: None,
            autoplay_import: true, remember_queue: true,
            background_on_close: true, volume: 0.82,
            shuffle: false, repeat: Repeat::Off, eq_gains: [0.0; 10],
            eq_preset: EqPreset::Flat,
            window_width: 600, window_height: 455,
        }
    }
}

pub fn config_file() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("slateamp").join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        fs::read(config_file()).ok()
            .and_then(|data| Self::from_json_bytes(&data))
            .unwrap_or_default()
    }

    fn from_json_bytes(data: &[u8]) -> Option<Self> {
        let mut settings = serde_json::from_slice::<Self>(data).ok()?;
        // In older releases only eq_gains was stored. Preserve the actual
        // sound and recover the name if it matches a built-in preset.
        let missing_preset = serde_json::from_slice::<serde_json::Value>(&data)
            .ok().is_some_and(|json| json.get("eq_preset").is_none());
        if missing_preset {
            settings.eq_preset = EqPreset::infer_from_gains(&settings.eq_gains);
        }
        Some(settings)
    }

    pub fn save(&self) -> Result<()> {
        let path = config_file();
        fs::create_dir_all(path.parent().context("No settings directory")?)?;
        let temp = path.with_extension("tmp");
        fs::write(&temp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(temp, path)?;
        Ok(())
    }
}

pub fn translate(lang: Language, english: &'static str, russian: &'static str) -> &'static str {
    let use_russian = match lang {
        Language::Russian => true,
        Language::English => false,
        Language::System => std::env::var("LANG").unwrap_or_default().to_lowercase().starts_with("ru"),
    };
    if use_russian { russian } else { english }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_have_stable_dropdown_indices() {
        for n in 0..=10 {
            assert_eq!(EqPreset::from_index(n).index(), n);
        }
    }

    #[test]
    fn old_settings_recover_named_rock_preset() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value.as_object_mut().unwrap().remove("eq_preset");
        // A pre-0.9 settings file could contain these Rock gains.
        value["eq_gains"] = serde_json::to_value(EqPreset::Rock.gains().unwrap()).unwrap();
        let json = serde_json::to_vec(&value).unwrap();
        let parsed = Settings::from_json_bytes(&json).unwrap();
        assert_eq!(parsed.eq_preset, EqPreset::Rock);
        assert_eq!(parsed.eq_gains, EqPreset::Rock.gains().unwrap());
    }
}
