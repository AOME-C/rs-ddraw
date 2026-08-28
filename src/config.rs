/// Runtime configuration loaded from ddraw.ini.
pub(crate) struct Config {
    pub windowed: bool,
    pub fullscreen: bool,
    pub maintas: bool,
    pub maxfps: i32,
    pub vsync: bool,
    pub adjmouse: bool,
    pub renderer: String,
    pub border: bool,
    pub devmode: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            windowed: false,
            fullscreen: false,
            maintas: false,
            maxfps: -1,
            vsync: false,
            adjmouse: true,
            renderer: "auto".into(),
            border: true,
            devmode: false,
        }
    }
}

/// Load configuration from ddraw.ini.
pub(crate) fn load() -> Config {
    // TODO: parse INI file, apply game-specific presets
    Config::default()
}

/// Save configuration to ddraw.ini.
pub(crate) fn save(_config: &Config) {
    // TODO: write current settings back to INI
}
