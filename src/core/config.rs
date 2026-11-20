// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

use std::convert::TryInto;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::fs::File;
use std::path::PathBuf;
use rgb::alt::BGR;
use rgb::alt::BGRA;
use rgb::RGB;
use yaml_rust2::yaml::Hash;
use yaml_rust2::yaml::LoadError;
use yaml_rust2::yaml::YamlDecoder;
use yaml_rust2::yaml::Yaml;

// Relative to $XDG_CONFIG_HOME (~/.config) and $XDG_CONFIG_DIRS (/etc/xdg).
const CONFIG_FILE: &str = "ufkbd/ufkbd.yaml";
// The old name, still read so existing configurations keep working.
const LEGACY_CONFIG_FILE: &str = "unfettered-keyboard.yaml";

#[derive(Debug)]
pub enum ConfigurationError {
    YamlError(LoadError),
    TypeError {
        field: &'static str,
        expected: &'static str,
    },
}

impl Display for ConfigurationError {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), std::fmt::Error>
    {
        match self {
            Self::YamlError(LoadError::Scan(e)) => {
                let marker = e.marker();
                write!(fmt, "ufkbd.yaml:{}:{}: {}",
                       marker.line(), marker.col(), e.info())?;
            },
            Self::YamlError(e) => {
                write!(fmt, "loading configuration: {:?}", e)?;
            },
            Self::TypeError { field, expected } => {
                write!(fmt, "reading configuration field {}: could not be converted to {}",
                        field, expected)?;
            },
        }
        Ok(())
    }
}

impl std::error::Error for ConfigurationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)>
    {
        match self {
            Self::YamlError(LoadError::IO(e)) => Some(e),
            Self::YamlError(LoadError::Scan(e)) => Some(e),
            _ => None,
        }
    }
}

impl From<LoadError> for ConfigurationError {
    fn from(e: LoadError) -> Self
    {
        Self::YamlError(e)
    }
}

#[derive(Debug, Clone)]
pub struct KeyboardColors {
    pub background: BGRA<u8>,
    pub keycap: BGRA<u8>,
    pub keycap_pressed: BGRA<u8>,
    pub label: BGR<u8>,
    pub label_locked: BGR<u8>,
    pub label_pressed: BGR<u8>,
    pub sublabel: BGR<u8>,
}

impl Default for KeyboardColors {
    fn default() -> Self
    {
        Self {
            background: RGB::new(15, 15, 15).into(),
            keycap: RGB::new(45, 45, 45).into(),
            keycap_pressed: RGB::new(77, 77, 77).into(),
            label: RGB::new(255, 255, 255).into(),
            label_locked: RGB::new(0, 255, 25).into(),
            label_pressed: RGB::new(0, 153, 255).into(),
            sublabel: RGB::new(178, 178, 178).into(),
        }
    }
}

fn parse_color(yaml: &Yaml) -> Option<RGB<u8>>
{
    let mut color_bytes = yaml
        .as_vec()?
        .iter()
        .map_while(|b| b.as_i64())
        .map_while(|b| b.try_into().ok());

    Some(RGB::new(
        color_bytes.next()?,
        color_bytes.next()?,
        color_bytes.next()?
    ))
}

#[derive(Debug)]
pub struct Configuration {
    longpress: u64,
    repeat: u64,
    haptics: bool,
    layout: String,
    extra_keys: Vec<String>,
    evfb_height: f64,
    wayland_height: i32,
    wayland_im_enable: bool,

    colors: KeyboardColors,
    keycap_padding: f64,
}

impl Configuration {
    fn load_evfb(&mut self, yaml: &Hash) -> Result<(), ConfigurationError>
    {
        let height = yaml.get(&Yaml::String(String::from("height")));
        if let Some(height) = height {
            // The keyboard takes this fraction of the screen, so 0 < height <= 1.
            let height = height.as_f64()
                .or_else(|| height.as_i64().map(|h| h as f64))
                .filter(|&h| h > 0.0 && h <= 1.0)
                .ok_or(ConfigurationError::TypeError {
                    field: "evfb.height",
                    expected: "fraction greater than 0 and at most 1",
                })?;
            self.evfb_height = height;
        }

        Ok(())
    }

    fn load_wayland(&mut self, yaml: &Hash) -> Result<(), ConfigurationError>
    {
        let height = yaml.get(&Yaml::String(String::from("height")));
        if let Some(height) = height {
            let height = match height.as_i64() {
                Some(h) => h.try_into().ok(),
                None => None,
            };
            let height = height.ok_or(ConfigurationError::TypeError {
                field: "wayland.height",
                expected: "31-bit integer",
            })?;
            self.wayland_height = height;
        }

        let im_enable = yaml.get(&Yaml::String(String::from("input-method")));
        if let Some(im_enable) = im_enable {
            let im_enable = im_enable.as_bool();
            let im_enable = im_enable.ok_or(ConfigurationError::TypeError {
                field: "wayland.input-method",
                expected: "boolean",
            })?;
            self.wayland_im_enable = im_enable;
        }

        Ok(())
    }

    fn load_colors(&mut self, yaml: &Hash) -> Result<(), ConfigurationError>
    {
        if let Some(keycap) = yaml.get(&Yaml::String(String::from("keycap"))) {
            self.colors.keycap = parse_color(keycap)
                .ok_or(ConfigurationError::TypeError {
                    field: "colors.keycap",
                    expected: "list of 8-bit unsigned integers",
                })?
                .into();
        }
        if let Some(keycap_pressed) = yaml.get(&Yaml::String(String::from("keycap_pressed"))) {
            self.colors.keycap_pressed = parse_color(keycap_pressed)
                .ok_or(ConfigurationError::TypeError {
                    field: "colors.keycap_pressed",
                    expected: "list of 8-bit unsigned integers",
                })?
                .into();
        }
        if let Some(label) = yaml.get(&Yaml::String(String::from("label"))) {
            self.colors.label = parse_color(label)
                .ok_or(ConfigurationError::TypeError {
                    field: "colors.label",
                    expected: "list of 8-bit unsigned integers",
                })?
                .into();
        }
        if let Some(label_pressed) = yaml.get(&Yaml::String(String::from("label_pressed"))) {
            self.colors.label_pressed = parse_color(label_pressed)
                .ok_or(ConfigurationError::TypeError {
                    field: "colors.label_pressed",
                    expected: "list of 8-bit unsigned integers",
                })?
                .into();
        }
        if let Some(label_locked) = yaml.get(&Yaml::String(String::from("label_locked"))) {
            self.colors.label_locked = parse_color(label_locked)
                .ok_or(ConfigurationError::TypeError {
                    field: "colors.label_locked",
                    expected: "list of 8-bit unsigned integers",
                })?
                .into();
        }
        if let Some(sublabel) = yaml.get(&Yaml::String(String::from("sublabel"))) {
            self.colors.sublabel = parse_color(sublabel)
                .ok_or(ConfigurationError::TypeError {
                    field: "colors.sublabel",
                    expected: "list of 8-bit unsigned integers",
                })?
                .into();
        }
        if let Some(background) = yaml.get(&Yaml::String(String::from("background"))) {
            self.colors.background = parse_color(background)
                .ok_or(ConfigurationError::TypeError {
                    field: "colors.background",
                    expected: "list of 8-bit unsigned integers",
                })?
                .into();
        }

        Ok(())
    }

    /*
     * Find the configuration file: ufkbd/ufkbd.yaml in the user's config
     * directory, then in the system-wide ones (e.g. /etc/xdg). The file's old
     * name is still read, but a user file always wins over a system one.
     */
    fn find_configuration_file() -> Option<PathBuf>
    {
        let xdg_dirs = xdg::BaseDirectories::new();
        let dirs = xdg_dirs.get_config_home().into_iter()
                           .chain(xdg_dirs.get_config_dirs());

        Self::find_configuration_file_in(dirs)
    }

    // Directories are searched in order, most important first.
    fn find_configuration_file_in<I>(dirs: I) -> Option<PathBuf>
        where I: IntoIterator<Item = PathBuf>
    {
        for dir in dirs {
            let path = dir.join(CONFIG_FILE);
            if path.is_file() {
                return Some(path);
            }

            let legacy = dir.join(LEGACY_CONFIG_FILE);
            if legacy.is_file() {
                eprintln!("warn: {} is deprecated, move it to {}",
                          legacy.display(), path.display());
                return Some(legacy);
            }
        }

        None
    }

    fn get_configuration_file() -> Result<File, Box<dyn std::error::Error>>
    {
        let config_file = Self::find_configuration_file()
            .ok_or(std::io::Error::new(std::io::ErrorKind::NotFound, "Config file not found"))?;
        Ok(File::open(config_file)?)
    }

    fn defaults() -> Self
    {
        Configuration {
            longpress: 600,
            repeat: 25,
            haptics: true,
            layout: String::from("latn_qwerty_us.xml"),
            extra_keys: vec![
                String::from("alt"),
                String::from("esc"),
                String::from("meta"),
                String::from("tab"),
            ],
            wayland_height: 185,
            wayland_im_enable: true,
            evfb_height: 0.25,

            colors: KeyboardColors::default(),
            keycap_padding: 0.02,
        }
    }

    pub fn load() -> Result<Self, ConfigurationError>
    {
        let mut cfg = Self::defaults();

        if let Ok(file) = Self::get_configuration_file() {
            let docs = YamlDecoder::read(file).decode()?;
            cfg.apply_documents(&docs)?;
        }

        Ok(cfg)
    }

    // Configuration from YAML text, for tests.
    #[cfg(test)]
    pub fn from_yaml_str(text: &str) -> Result<Self, ConfigurationError>
    {
        let docs = YamlDecoder::read(text.as_bytes()).decode()?;
        let mut cfg = Self::defaults();
        cfg.apply_documents(&docs)?;
        Ok(cfg)
    }

    /*
     * Apply the settings of a decoded configuration file. A file that is
     * empty, has only comments, or is only a document header has no
     * settings and keeps the defaults.
     */
    fn apply_documents(&mut self, docs: &[Yaml]) -> Result<(), ConfigurationError>
    {
        let yaml = match docs.first() {
            None | Some(Yaml::Null) => return Ok(()),
            Some(doc) => doc.as_hash().ok_or(ConfigurationError::TypeError {
                field: "(top level)",
                expected: "YAML mapping",
            })?,
        };

        let longpress = yaml.get(&Yaml::String(String::from("longpress_ms")));
        if let Some(longpress) = longpress {
            let longpress = match longpress.as_i64() {
                Some(l) => l.try_into().ok(),
                None => None,
            };
            let longpress = longpress.ok_or(ConfigurationError::TypeError {
                field: "longpress_ms",
                expected: "unsigned integer",
            })?;
            self.longpress = longpress;
        }

        let repeat = yaml.get(&Yaml::String(String::from("repeat_ms")));
        if let Some(repeat) = repeat {
            let repeat = match repeat.as_i64() {
                Some(l) => l.try_into().ok(),
                None => None,
            };
            // A zero interval would repeat a held key endlessly without pausing.
            let repeat = repeat.filter(|&r: &u64| r >= 1).ok_or(ConfigurationError::TypeError {
                field: "repeat_ms",
                expected: "integer of at least 1",
            })?;
            self.repeat = repeat;
        }

        let haptics = yaml.get(&Yaml::String(String::from("haptics")));
        if let Some(haptics) = haptics {
            let haptics = haptics.as_bool().ok_or(ConfigurationError::TypeError {
                field: "haptics",
                expected: "boolean",
            })?;
            self.haptics = haptics;
        }

        let layout = yaml.get(&Yaml::String(String::from("layout")));
        if let Some(layout) = layout {
            let layout = layout.as_str().ok_or(ConfigurationError::TypeError {
                field: "layout",
                expected: "string",
            })?;
            self.layout = layout.to_string();
        }

        let keys = yaml.get(&Yaml::String(String::from("extra_keys")));
        if let Some(keys) = keys {
            let keys = keys.as_vec().ok_or(ConfigurationError::TypeError {
                field: "extra_keys",
                expected: "list",
            })?;
            self.extra_keys = Vec::with_capacity(keys.len());

            for k in keys {
                let k = k.as_str().ok_or(ConfigurationError::TypeError {
                    field: "extra_keys",
                    expected: "list of strings",
                })?;
                self.extra_keys.push(String::from(k));
            }

            self.extra_keys.sort_unstable();
        }

        let wl = yaml.get(&Yaml::String(String::from("wayland")));
        if let Some(wl) = wl {
            let wl = wl.as_hash().ok_or(ConfigurationError::TypeError {
                field: "wayland",
                expected: "YAML mapping",
            })?;
            self.load_wayland(wl)?;
        }

        let evfb = yaml.get(&Yaml::String(String::from("evfb")));
        if let Some(evfb) = evfb {
            let evfb = evfb.as_hash().ok_or(ConfigurationError::TypeError {
                field: "evfb",
                expected: "YAML mapping",
            })?;
            self.load_evfb(evfb)?;
        }

        let colors = yaml.get(&Yaml::String(String::from("colors")));
        if let Some(colors) = colors {
            let colors = colors.as_hash().ok_or(ConfigurationError::TypeError {
                field: "colors",
                expected: "YAML mapping",
            })?;
            self.load_colors(colors)?;
        }

        let keycap_padding = yaml.get(&Yaml::String(String::from("keycap_padding")));
        if let Some(keycap_padding) = keycap_padding {
            let keycap_padding = keycap_padding.as_f64().ok_or(ConfigurationError::TypeError {
                field: "keycap_padding",
                expected: "fraction",
            })?;
            self.keycap_padding = keycap_padding;
        }

        Ok(())
    }

    #[inline(always)]
    pub fn longpress_ms(&self) -> u64
    {
        self.longpress
    }

    #[inline(always)]
    pub fn repeat_ms(&self) -> u64
    {
        self.repeat
    }

    #[inline(always)]
    pub fn haptics(&self) -> bool
    {
        self.haptics
    }

    #[inline(always)]
    pub fn layout(&self) -> &str
    {
        &self.layout
    }

    #[inline(always)]
    pub fn extra_keys(&self) -> &Vec<String>
    {
        &self.extra_keys
    }

    #[inline(always)]
    pub fn wayland_height(&self) -> i32
    {
        self.wayland_height
    }

    #[inline(always)]
    pub fn wayland_im_enable(&self) -> bool
    {
        self.wayland_im_enable
    }

    #[inline(always)]
    pub fn evfb_height(&self) -> f64
    {
        self.evfb_height
    }

    #[inline(always)]
    pub fn colors(&self) -> &KeyboardColors
    {
        &self.colors
    }

    #[inline(always)]
    pub fn keycap_padding(&self) -> f64
    {
        self.keycap_padding
    }
}

#[cfg(test)]
mod tests {
    use super::Configuration;
    use std::fs;
    use std::path::PathBuf;

    fn parse(text: &str) -> Result<Configuration, super::ConfigurationError>
    {
        Configuration::from_yaml_str(text)
    }

    #[test]
    fn empty_files_use_defaults()
    {
        for text in ["", "\n", "# only a comment\n", "%YAML 1.2\n---\n", "---\n"] {
            let cfg = parse(text).unwrap_or_else(|e| panic!("{:?}: {}", text, e));
            assert_eq!(cfg.repeat_ms(), 25, "{:?}", text);
        }
    }

    #[test]
    fn repeat_ms_must_be_positive()
    {
        assert!(parse("repeat_ms: 0").is_err());
        assert!(parse("repeat_ms: -5").is_err());
        assert_eq!(parse("repeat_ms: 1").unwrap().repeat_ms(), 1);
    }

    #[test]
    fn evfb_height_must_be_a_fraction()
    {
        assert!(parse("evfb: { height: 0 }").is_err());
        assert!(parse("evfb: { height: 1.5 }").is_err());
        assert!(parse("evfb: { height: -0.2 }").is_err());
        assert_eq!(parse("evfb: { height: 1 }").unwrap().evfb_height(), 1.0);
        assert_eq!(parse("evfb: { height: 0.3 }").unwrap().evfb_height(), 0.3);
    }

    #[test]
    fn non_mapping_is_an_error()
    {
        assert!(parse("- a list").is_err());
    }

    #[test]
    fn configuration_file_lookup_order()
    {
        let root = std::env::temp_dir().join(format!("ufkbd-config-test-{}", std::process::id()));
        let (home, etc) = (root.join("home"), root.join("etc"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(home.join("ufkbd")).unwrap();
        fs::create_dir_all(etc.join("ufkbd")).unwrap();

        let find = || Configuration::find_configuration_file_in(vec![home.clone(), etc.clone()]);
        let touch = |p: PathBuf| { fs::write(&p, "").unwrap(); p };

        assert_eq!(find(), None);

        let etc_legacy = touch(etc.join("unfettered-keyboard.yaml"));
        assert_eq!(find(), Some(etc_legacy));

        let etc_new = touch(etc.join("ufkbd/ufkbd.yaml"));
        assert_eq!(find(), Some(etc_new));

        // A user file with the old name still wins over the system's file.
        let home_legacy = touch(home.join("unfettered-keyboard.yaml"));
        assert_eq!(find(), Some(home_legacy));

        let home_new = touch(home.join("ufkbd/ufkbd.yaml"));
        assert_eq!(find(), Some(home_new));

        let _ = fs::remove_dir_all(&root);
    }
}
