//! Lighting effects, the controller actions that produce them, and the saved settings.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::controller::{Action, MODE_COLOR, MODE_MORPH, MODE_PULSE, Rgb};

const BLACK: Rgb = [0, 0, 0];
const MORPH_TEMPO: u16 = 64;
const WAVE_PHASES: usize = 4;

pub const SPECTRUM: [Rgb; 7] = [
    [0xFF, 0x00, 0x00],
    [0xFF, 0x7A, 0x00],
    [0xFF, 0xFF, 0x00],
    [0x00, 0xD9, 0x5F],
    [0x00, 0xBF, 0xFF],
    [0x24, 0x5B, 0xFF],
    [0x8A, 0x2B, 0xE2],
];

pub const PALETTE: [(&str, Rgb); 16] = [
    ("Red", [0xFF, 0x00, 0x00]),
    ("Crimson", [0xC0, 0x10, 0x30]),
    ("Orange", [0xFF, 0x65, 0x00]),
    ("Amber", [0xFF, 0xA0, 0x00]),
    ("Yellow", [0xFF, 0xE0, 0x00]),
    ("Lime", [0x80, 0xFF, 0x00]),
    ("Green", [0x00, 0xFF, 0x40]),
    ("Teal", [0x00, 0xC8, 0x96]),
    ("Cyan", [0x00, 0xE5, 0xFF]),
    ("Sky", [0x00, 0x8C, 0xFF]),
    ("Blue", [0x00, 0x00, 0xFF]),
    ("Indigo", [0x4B, 0x00, 0xFF]),
    ("Violet", [0x8A, 0x2B, 0xE2]),
    ("Magenta", [0xFF, 0x00, 0xFF]),
    ("Pink", [0xFF, 0x4F, 0x9A]),
    ("White", [0xFF, 0xFF, 0xFF]),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    Static,
    Breathe,
    Pulse,
    Morph,
    Spectrum,
    Rainbow,
    Off,
}

impl Effect {
    pub const ALL: [Effect; 7] = [
        Effect::Static,
        Effect::Breathe,
        Effect::Pulse,
        Effect::Morph,
        Effect::Spectrum,
        Effect::Rainbow,
        Effect::Off,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Effect::Static => "Static",
            Effect::Breathe => "Breathe",
            Effect::Pulse => "Pulse",
            Effect::Morph => "Morph",
            Effect::Spectrum => "Spectrum",
            Effect::Rainbow => "Rainbow wave",
            Effect::Off => "Off",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Effect::Static => "One solid color on every zone.",
            Effect::Breathe => "Fades a color in and out.",
            Effect::Pulse => "Blinks a color on and off.",
            Effect::Morph => "Fades back and forth between two colors.",
            Effect::Spectrum => "All zones cycle through the rainbow together.",
            Effect::Rainbow => "The rainbow ripples across the zones.",
            Effect::Off => "Turns the chassis lighting off.",
        }
    }

    /// How many user-chosen colors the effect uses.
    pub fn color_count(self) -> usize {
        match self {
            Effect::Static | Effect::Breathe | Effect::Pulse => 1,
            Effect::Morph => 2,
            Effect::Spectrum | Effect::Rainbow | Effect::Off => 0,
        }
    }

    pub fn uses_speed(self) -> bool {
        !matches!(self, Effect::Static | Effect::Off)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Speed {
    Slow,
    Medium,
    Fast,
}

impl Speed {
    pub const ALL: [Speed; 3] = [Speed::Slow, Speed::Medium, Speed::Fast];

    pub fn label(self) -> &'static str {
        match self {
            Speed::Slow => "Slow",
            Speed::Medium => "Medium",
            Speed::Fast => "Fast",
        }
    }

    /// Milliseconds per animation step.
    fn duration(self) -> u16 {
        match self {
            Speed::Slow => 3500,
            Speed::Medium => 1800,
            Speed::Fast => 800,
        }
    }

    fn pulse_tempo(self) -> u16 {
        match self {
            Speed::Slow => 100,
            Speed::Medium => 175,
            Speed::Fast => 250,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub effect: Effect,
    pub colors: [Rgb; 2],
    pub speed: Speed,
    /// 0-100 percent.
    pub brightness: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            effect: Effect::Rainbow,
            colors: [[0x00, 0x00, 0xFF], [0xFF, 0x00, 0xFF]],
            speed: Speed::Medium,
            brightness: 100,
        }
    }
}

impl Settings {
    /// One looping action sequence per phase (see `Controller::play`).
    pub fn sequences(&self) -> Vec<Vec<Action>> {
        let duration = self.speed.duration();
        let morph = |color| Action {
            mode: MODE_MORPH,
            duration,
            tempo: MORPH_TEMPO,
            color,
        };
        let [first, second] = self.colors;
        match self.effect {
            Effect::Static => vec![vec![Action {
                mode: MODE_COLOR,
                duration: 1,
                tempo: 2,
                color: first,
            }]],
            Effect::Off => vec![vec![Action {
                mode: MODE_COLOR,
                duration: 1,
                tempo: 2,
                color: BLACK,
            }]],
            Effect::Breathe => vec![vec![morph(first), morph(BLACK)]],
            Effect::Pulse => vec![vec![Action {
                mode: MODE_PULSE,
                duration,
                tempo: self.speed.pulse_tempo(),
                color: first,
            }]],
            Effect::Morph => vec![vec![morph(first), morph(second)]],
            Effect::Spectrum => vec![SPECTRUM.iter().copied().map(morph).collect()],
            Effect::Rainbow => (0..WAVE_PHASES)
                .map(|phase| {
                    let offset = (phase * SPECTRUM.len() + WAVE_PHASES / 2) / WAVE_PHASES;
                    let mut colors = SPECTRUM;
                    colors.rotate_left(offset % SPECTRUM.len());
                    colors.into_iter().map(morph).collect()
                })
                .collect(),
        }
    }

    pub fn brightness_for_device(&self) -> u8 {
        if self.effect == Effect::Off {
            0
        } else {
            self.brightness.min(100)
        }
    }

    /// Approximate on-screen color of zone `zone` at `millis` into the animation.
    pub fn preview_color(&self, sequences: &[Vec<Action>], zone: usize, millis: u64) -> Rgb {
        let actions = &sequences[zone % sequences.len()];
        let color = match actions[0].mode {
            MODE_COLOR => actions[0].color,
            MODE_PULSE => {
                let period = u64::from(actions[0].duration.max(1));
                let on = (millis % period) < period / 2;
                if on { actions[0].color } else { BLACK }
            }
            _ => {
                let total: u64 = actions.iter().map(|a| u64::from(a.duration.max(1))).sum();
                let mut t = millis % total;
                let mut current = actions.len() - 1;
                for (i, action) in actions.iter().enumerate() {
                    let d = u64::from(action.duration.max(1));
                    if t < d {
                        current = i;
                        break;
                    }
                    t -= d;
                }
                let from = actions[(current + actions.len() - 1) % actions.len()].color;
                let to = actions[current].color;
                let frac = t as f32 / f32::from(actions[current].duration.max(1));
                lerp(from, to, frac)
            }
        };
        let scale = f32::from(self.brightness_for_device()) / 100.0;
        color.map(|c| (f32::from(c) * scale).round() as u8)
    }

    fn path() -> Option<PathBuf> {
        Some(config_home()?.join("aurora-lights").join("settings.json"))
    }

    pub fn load() -> Option<Settings> {
        let text = fs::read_to_string(Self::path()?).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or("cannot locate the config directory")?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))
    }
}

/// Per-user config directory: `$XDG_CONFIG_HOME` or `~/.config` on Linux, `%APPDATA%` on Windows.
pub fn config_home() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("APPDATA").map(PathBuf::from);
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

fn lerp(from: Rgb, to: Rgb, frac: f32) -> Rgb {
    let frac = frac.clamp(0.0, 1.0);
    std::array::from_fn(|i| {
        (f32::from(from[i]) + (f32::from(to[i]) - f32::from(from[i])) * frac).round() as u8
    })
}
