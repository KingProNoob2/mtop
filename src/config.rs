use crate::processes::{ProcessFilter, SortColumn};
use crate::ui::{Theme, ViewMode};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SavedScreen {
    #[default]
    Split,
    Processes,
    Hardware,
}

impl SavedScreen {
    pub fn next(self) -> Self {
        match self {
            SavedScreen::Split => SavedScreen::Processes,
            SavedScreen::Processes => SavedScreen::Hardware,
            SavedScreen::Hardware => SavedScreen::Split,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            SavedScreen::Split => SavedScreen::Hardware,
            SavedScreen::Processes => SavedScreen::Split,
            SavedScreen::Hardware => SavedScreen::Processes,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SavedScreen::Split => "Split",
            SavedScreen::Processes => "Processes",
            SavedScreen::Hardware => "Hardware",
        }
    }

    pub fn to_view_mode(self) -> ViewMode {
        match self {
            SavedScreen::Split => ViewMode::Combined,
            SavedScreen::Processes => ViewMode::Processes,
            SavedScreen::Hardware => ViewMode::Hardware,
        }
    }

    pub fn from_view_mode(v: ViewMode) -> Self {
        match v {
            ViewMode::Combined => SavedScreen::Split,
            ViewMode::Processes => SavedScreen::Processes,
            ViewMode::Hardware => SavedScreen::Hardware,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SavedTheme {
    #[default]
    Cyan,
    Green,
    Amber,
    Magenta,
}

impl From<Theme> for SavedTheme {
    fn from(t: Theme) -> Self {
        match t {
            Theme::Cyan => SavedTheme::Cyan,
            Theme::Green => SavedTheme::Green,
            Theme::Amber => SavedTheme::Amber,
            Theme::Magenta => SavedTheme::Magenta,
        }
    }
}

impl From<SavedTheme> for Theme {
    fn from(t: SavedTheme) -> Self {
        match t {
            SavedTheme::Cyan => Theme::Cyan,
            SavedTheme::Green => Theme::Green,
            SavedTheme::Amber => Theme::Amber,
            SavedTheme::Magenta => Theme::Magenta,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SavedSortColumn {
    #[default]
    Cpu,
    Memory,
    NetDown,
    NetUp,
    App,
    Disk,
    Pid,
    Threads,
}

impl From<SortColumn> for SavedSortColumn {
    fn from(s: SortColumn) -> Self {
        match s {
            SortColumn::Cpu => SavedSortColumn::Cpu,
            SortColumn::Memory => SavedSortColumn::Memory,
            SortColumn::NetDown => SavedSortColumn::NetDown,
            SortColumn::NetUp => SavedSortColumn::NetUp,
            SortColumn::App => SavedSortColumn::App,
            SortColumn::Disk => SavedSortColumn::Disk,
            SortColumn::Pid => SavedSortColumn::Pid,
            SortColumn::Threads => SavedSortColumn::Threads,
        }
    }
}

impl From<SavedSortColumn> for SortColumn {
    fn from(s: SavedSortColumn) -> Self {
        match s {
            SavedSortColumn::Cpu => SortColumn::Cpu,
            SavedSortColumn::Memory => SortColumn::Memory,
            SavedSortColumn::NetDown => SortColumn::NetDown,
            SavedSortColumn::NetUp => SortColumn::NetUp,
            SavedSortColumn::App => SortColumn::App,
            SavedSortColumn::Disk => SortColumn::Disk,
            SavedSortColumn::Pid => SortColumn::Pid,
            SavedSortColumn::Threads => SortColumn::Threads,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub remember_last_state: bool,
    pub default_screen: SavedScreen,
    pub last_screen: SavedScreen,
    pub theme: SavedTheme,
    pub interval_ms: u64,
    pub sort_col: SavedSortColumn,
    pub sort_desc: bool,
    pub apps_only: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            remember_last_state: true,
            default_screen: SavedScreen::Split,
            last_screen: SavedScreen::Split,
            theme: SavedTheme::Cyan,
            interval_ms: 1000,
            sort_col: SavedSortColumn::Cpu,
            sort_desc: true,
            apps_only: false,
        }
    }
}

impl AppConfig {
    pub fn config_path() -> Option<PathBuf> {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            let mut p = PathBuf::from(xdg);
            p.push("mtop");
            p.push("config.json");
            return Some(p);
        }
        std::env::var_os("HOME").map(|home| {
            let mut p = PathBuf::from(home);
            p.push(".config");
            p.push("mtop");
            p.push("config.json");
            p
        })
    }

    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        let Ok(raw) = fs::read_to_string(&path) else {
            return Self::default();
        };
        let mut cfg: Self = serde_json::from_str(&raw).unwrap_or_default();
        cfg.interval_ms = cfg.interval_ms.clamp(250, 10_000);
        cfg
    }

    pub fn save(&self) {
        let Some(path) = Self::config_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, json);
        }
    }

    pub fn initial_view_mode(&self) -> ViewMode {
        if self.remember_last_state {
            self.last_screen.to_view_mode()
        } else {
            self.default_screen.to_view_mode()
        }
    }

    pub fn initial_filter(&self) -> ProcessFilter {
        if self.apps_only {
            ProcessFilter::AppsOnly
        } else {
            ProcessFilter::All
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_screen_respects_remember_last_state() {
        let mut cfg = AppConfig::default();
        cfg.default_screen = SavedScreen::Hardware;
        cfg.last_screen = SavedScreen::Processes;

        cfg.remember_last_state = true;
        assert_eq!(cfg.initial_view_mode(), ViewMode::Processes);

        cfg.remember_last_state = false;
        assert_eq!(cfg.initial_view_mode(), ViewMode::Hardware);
    }

    #[test]
    fn config_roundtrip_json() {
        let mut cfg = AppConfig::default();
        cfg.remember_last_state = false;
        cfg.default_screen = SavedScreen::Hardware;
        cfg.theme = SavedTheme::Amber;
        cfg.interval_ms = 500;
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: AppConfig = serde_json::from_str(&json).unwrap();
        assert!(!parsed.remember_last_state);
        assert_eq!(parsed.default_screen, SavedScreen::Hardware);
        assert_eq!(parsed.theme, SavedTheme::Amber);
        assert_eq!(parsed.interval_ms, 500);
    }
}
