// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

use crate::core::Configuration;
use crate::core::Keyboard;
use crate::core::ModState;
use crate::core::expat;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::str::FromStr;
use std::ffi::CStr;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::fs::File;
use std::io::Read;
use std::os::raw::c_char;
use std::os::raw::c_int;
use std::os::raw::c_void;
use std::path::Path;
use std::path::PathBuf;
use std::ptr;
use xdg::BaseDirectories;
use xkeysym::Keysym;

pub enum LayoutError {
    NotFound(String),
    CannotOpen(PathBuf, std::io::Error),
    CannotRead(PathBuf, std::io::Error),
    OutOfMemory,
    ParseError(PathBuf, u32),
}

impl Debug for LayoutError {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), std::fmt::Error>
    {
        match self {
            Self::NotFound(f) => {
                write!(fmt, "{} not found", f)?;
            },
            Self::CannotOpen(f, e) => {
                write!(fmt, "could not open {:?}: {}", f, e)?;
            },
            Self::CannotRead(f, e) => {
                write!(fmt, "error reading {:?}: {}", f, e)?;
            },
            Self::OutOfMemory => {
                write!(fmt, "out of memory")?;
            },
            Self::ParseError(f, e) => {
                write!(fmt, "parsing {:?}: got expat error {}", f, e)?;
            },
        }
        Ok(())
    }
}

unsafe extern "C" fn start_elem(data: *mut c_void,
                                c_name: *const expat::XML_Char,
                                c_attrs: *mut *const expat::XML_Char)
{
    let layout = &mut *(data as *mut LayoutLoader);
    let name = CStr::from_ptr(c_name)
                     .to_str().expect("layout: XML element name must be UTF-8");

    let mut attrs = HashMap::with_capacity(10);

    while !ptr::eq(*c_attrs.add(attrs.len() * 2), ptr::null()) {
        let k = CStr::from_ptr(*c_attrs.add(attrs.len() * 2))
                          .to_str().expect("layout: XML attribute name must be UTF-8");
        let v = CStr::from_ptr(*c_attrs.add(attrs.len() * 2 + 1))
                          .to_str().expect("layout: XML attribute value must be UTF-8");

        attrs.insert(k, v);
    }

    layout.start_elem(name, &attrs);
}

unsafe extern "C" fn end_elem(data: *mut c_void, c_name: *const expat::XML_Char)
{
    let layout = &mut *(data as *mut LayoutLoader);
    let name = CStr::from_ptr(c_name)
                     .to_str().expect("layout: XML element name must be UTF-8");

    layout.end_elem(name);
}

struct ExpatParser(expat::XML_Parser);

impl ExpatParser {
    fn create() -> Result<Self, LayoutError>
    {
        let parser = unsafe { expat::XML_ParserCreate(ptr::null()) };

        if !parser.is_null() {
            Ok(Self(parser))
        } else {
            Err(LayoutError::OutOfMemory)
        }
    }

    fn parse<P: AsRef<Path>>(&mut self, path: P, xml: &[u8]) -> Result<(), LayoutError>
    {
        let status = unsafe { expat::XML_Parse(self.0,
                                               xml.as_ptr() as *const c_char,
                                               xml.len() as c_int, 1) };
        if status != expat::XML_Status_XML_STATUS_OK {
            let err = unsafe { expat::XML_GetErrorCode(self.0) };

            Err(LayoutError::ParseError(path.as_ref().to_path_buf(), err))
        } else {
            Ok(())
        }
    }
}

impl Drop for ExpatParser {
    fn drop(&mut self)
    {
        unsafe { expat::XML_ParserFree(self.0) }
    }
}

pub const MOD_SHIFT: usize = 1;
pub const MOD_CTRL: usize = 2;
pub const MOD_ALT: usize = 3;
pub const MOD_FN: usize = 4;
pub const MOD_META: usize = 5;
pub const MODIFIERS_MAX: usize = 5;

#[derive(Clone)]
struct KeyValue(Keysym, String);

impl KeyValue {
    fn from(key: Keysym, text: &str) -> KeyValue
    {
        KeyValue(key, String::from(text))
    }
}

#[derive(Clone)]
pub struct Part {
    orig: KeyValue,
    val: KeyValue,

    key_avail: bool,
    text_avail: bool,

    presses: u32,
}

impl Part {
    pub fn modify_shift(orig_sym: Keysym) -> Keysym
    {
        match orig_sym {
            Keysym::a => Keysym::A,
            Keysym::b => Keysym::B,
            Keysym::c => Keysym::C,
            Keysym::d => Keysym::D,
            Keysym::e => Keysym::E,
            Keysym::f => Keysym::F,
            Keysym::g => Keysym::G,
            Keysym::h => Keysym::H,
            Keysym::i => Keysym::I,
            Keysym::j => Keysym::J,
            Keysym::k => Keysym::K,
            Keysym::l => Keysym::L,
            Keysym::m => Keysym::M,
            Keysym::n => Keysym::N,
            Keysym::o => Keysym::O,
            Keysym::p => Keysym::P,
            Keysym::q => Keysym::Q,
            Keysym::r => Keysym::R,
            Keysym::s => Keysym::S,
            Keysym::t => Keysym::T,
            Keysym::u => Keysym::U,
            Keysym::v => Keysym::V,
            Keysym::w => Keysym::W,
            Keysym::x => Keysym::X,
            Keysym::y => Keysym::Y,
            Keysym::z => Keysym::Z,

            Keysym::ae => Keysym::AE,
            Keysym::oe => Keysym::OE,
            Keysym::mu => Keysym::Greek_MU,

            _ => orig_sym,
        }
    }

    fn modify_shift_label(orig_sym: &KeyValue) -> KeyValue
    {
        if Self::keyvalue_has_text(orig_sym) {
            let upper = orig_sym.1.to_uppercase();
            KeyValue(orig_sym.0, upper)
        } else {
            orig_sym.clone()
        }
    }

    fn modify_fn(orig_sym: &KeyValue) -> KeyValue
    {
        match orig_sym {
            KeyValue(Keysym::Tab, _)
                => KeyValue::from(Keysym::NoSymbol, "\t"),

            KeyValue(Keysym::Up, _)
                => KeyValue::from(Keysym::Prior, "⇑"),
            KeyValue(Keysym::Down, _)
                => KeyValue::from(Keysym::Next, "⇓"),
            KeyValue(Keysym::Left, _)
                => KeyValue::from(Keysym::Home, "⇤"),
            KeyValue(Keysym::Right, _)
                => KeyValue::from(Keysym::End, "⇥"),
            KeyValue(Keysym::Escape, _)
                => KeyValue::from(Keysym::Insert, "Ins"),

            KeyValue(Keysym::_1, _)
                => KeyValue::from(Keysym::F1, "F1"),
            KeyValue(Keysym::_2, _)
                => KeyValue::from(Keysym::F2, "F2"),
            KeyValue(Keysym::_3, _)
                => KeyValue::from(Keysym::F3, "F3"),
            KeyValue(Keysym::_4, _)
                => KeyValue::from(Keysym::F4, "F4"),
            KeyValue(Keysym::_5, _)
                => KeyValue::from(Keysym::F5, "F5"),
            KeyValue(Keysym::_6, _)
                => KeyValue::from(Keysym::F6, "F6"),
            KeyValue(Keysym::_7, _)
                => KeyValue::from(Keysym::F7, "F7"),
            KeyValue(Keysym::_8, _)
                => KeyValue::from(Keysym::F8, "F8"),
            KeyValue(Keysym::_9, _)
                => KeyValue::from(Keysym::F9, "F9"),
            KeyValue(Keysym::_0, _)
                => KeyValue::from(Keysym::F10, "F10"),

            KeyValue(Keysym::less, _)
                => KeyValue::from(Keysym::guillemetleft, "«"),
            KeyValue(Keysym::greater, _)
                => KeyValue::from(Keysym::guillemetright, "»"),
            KeyValue(Keysym::braceleft, _)
                => KeyValue::from(Keysym::NoSymbol, "‹"),
            KeyValue(Keysym::braceright, _)
                => KeyValue::from(Keysym::NoSymbol, "›"),
            KeyValue(Keysym::bracketleft, _)
                => KeyValue::from(Keysym::leftsinglequotemark, "‘"),
            KeyValue(Keysym::bracketright, _)
                => KeyValue::from(Keysym::rightsinglequotemark, "’"),
            KeyValue(Keysym::parenleft, _)
                => KeyValue::from(Keysym::leftdoublequotemark, "“"),
            KeyValue(Keysym::parenright, _)
                => KeyValue::from(Keysym::rightdoublequotemark, "”"),
            KeyValue(Keysym::apostrophe, _)
                => KeyValue::from(Keysym::singlelowquotemark, "‚"),
            KeyValue(Keysym::quotedbl, _)
                => KeyValue::from(Keysym::doublelowquotemark, "„"),
            KeyValue(Keysym::minus, _)
                => KeyValue::from(Keysym::endash, "–"),
            KeyValue(Keysym::underscore, _)
                => KeyValue::from(Keysym::emdash, "—"),
            KeyValue(Keysym::asciicircum, _)
                => KeyValue::from(Keysym::notsign, "¬"),
            KeyValue(Keysym::percent, _)
                => KeyValue::from(Keysym::permille, "‰"),
            KeyValue(Keysym::equal, _)
                => KeyValue::from(Keysym::approxeq, "≈"),
            KeyValue(Keysym::u, _)
                => KeyValue::from(Keysym::mu, "µ"),
            KeyValue(Keysym::a, _)
                => KeyValue::from(Keysym::ae, "æ"),
            KeyValue(Keysym::o, _)
                => KeyValue::from(Keysym::oe, "œ"),
            KeyValue(Keysym::asterisk, _)
                => KeyValue::from(Keysym::degree, "°"),
            KeyValue(Keysym::period, _)
                => KeyValue::from(Keysym::ellipsis, "…"),
            KeyValue(Keysym::comma, _)
                => KeyValue::from(Keysym::periodcentered, "·"),
            KeyValue(Keysym::exclam, _)
                => KeyValue::from(Keysym::exclamdown, "¡"),
            KeyValue(Keysym::question, _)
                => KeyValue::from(Keysym::questiondown, "¿"),
            KeyValue(Keysym::bar, _)
                => KeyValue::from(Keysym::brokenbar, "¦"),

            KeyValue(Keysym::e, _)
                => KeyValue::from(Keysym::EuroSign, "€"),
            KeyValue(Keysym::l, _)
                => KeyValue::from(Keysym::sterling, "£"),
            KeyValue(Keysym::r, _)
                => KeyValue::from(Keysym::NoSymbol, "₹"),
            KeyValue(Keysym::y, _)
                => KeyValue::from(Keysym::yen, "¥"),
            KeyValue(Keysym::c, _)
                => KeyValue::from(Keysym::cent, "¢"),
            KeyValue(Keysym::p, _)
                => KeyValue::from(Keysym::NoSymbol, "₽"),
            KeyValue(Keysym::b, _)
                => KeyValue::from(Keysym::NoSymbol, "₱"),
            KeyValue(Keysym::h, _)
                => KeyValue::from(Keysym::NoSymbol, "₴"),
            KeyValue(Keysym::z, _)
                => KeyValue::from(Keysym::NoSymbol, "₿"),

            _ => orig_sym.clone(),
        }
    }

    fn modify_no_fn(orig: &KeyValue) -> KeyValue
    {
        match orig {
            KeyValue(Keysym::F11, _) => KeyValue::from(Keysym::NoSymbol, ""),
            KeyValue(Keysym::F12, _) => KeyValue::from(Keysym::NoSymbol, ""),
            _ => orig.clone(),
        }
    }

    fn keyvalue_has_text(val: &KeyValue) -> bool
    {
        match val {
            KeyValue(Keysym::BackSpace, _) => false,
            KeyValue(Keysym::Tab, _) => false,
            KeyValue(Keysym::Return, _) => false,
            KeyValue(Keysym::Escape, _) => false,
            KeyValue(Keysym::Home, _) => false,
            KeyValue(Keysym::Left, _) => false,
            KeyValue(Keysym::Up, _) => false,
            KeyValue(Keysym::Right, _) => false,
            KeyValue(Keysym::Down, _) => false,
            KeyValue(Keysym::Prior, _) => false,
            KeyValue(Keysym::Next, _) => false,
            KeyValue(Keysym::End, _) => false,
            KeyValue(Keysym::Insert, _) => false,
            KeyValue(Keysym::F1, _) => false,
            KeyValue(Keysym::F2, _) => false,
            KeyValue(Keysym::F3, _) => false,
            KeyValue(Keysym::F4, _) => false,
            KeyValue(Keysym::F5, _) => false,
            KeyValue(Keysym::F6, _) => false,
            KeyValue(Keysym::F7, _) => false,
            KeyValue(Keysym::F8, _) => false,
            KeyValue(Keysym::F9, _) => false,
            KeyValue(Keysym::F10, _) => false,
            KeyValue(Keysym::F11, _) => false,
            KeyValue(Keysym::F12, _) => false,
            KeyValue(Keysym::Delete, _) => false,

            KeyValue(Keysym::Alt_L, _) => false,
            KeyValue(Keysym::Control_L, _) => false,
            KeyValue(Keysym::Meta_L, _) => false,
            KeyValue(Keysym::XF86_Fn, _) => false,

            KeyValue(Keysym::Scroll_Lock, _) => false,
            KeyValue(Keysym::Num_Lock, _) => false,
            KeyValue(Keysym::Caps_Lock, _) => false,

            KeyValue(_, l) => l != "",
        }
    }

    fn new(orig: KeyValue) -> Part
    {
        let val = Self::modify_no_fn(&orig);

        Part {
            orig,
            val,

            key_avail: false,
            text_avail: false,

            presses: 0,
        }
    }

    #[inline(always)]
    pub fn sym(&self) -> Keysym
    {
        self.val.0
    }

    #[inline(always)]
    pub fn text(&self) -> &str
    {
        &self.val.1
    }

    #[inline(always)]
    pub fn press(&mut self)
    {
        self.presses += 1;
    }

    #[inline(always)]
    pub fn release(&mut self)
    {
        self.presses -= 1;
    }

    #[inline(always)]
    pub fn presses(&self) -> u32
    {
        self.presses
    }

    #[inline(always)]
    pub fn text_available(&self) -> bool
    {
        self.text_avail
    }

    #[inline(always)]
    pub fn key_available(&self) -> bool
    {
        self.key_avail
    }

    fn set_text_supported(&mut self, text_supp: bool)
    {
        self.text_avail = text_supp && Self::keyvalue_has_text(&self.val);
    }

    fn update_keys_supported<K: Keyboard>(&mut self, kbd: &K)
    {
        let key_supp = kbd.key_supported(self.val.0);
        self.key_avail = self.val.0 != Keysym::NoSymbol && key_supp;
    }

    pub fn pressable(&self) -> bool
    {
        match self.val.0 {
            Keysym::Scroll_Lock => true,
            Keysym::Num_Lock => true,
            Keysym::Caps_Lock => true,

            Keysym::Meta_L => true,

            _ => self.text_avail || self.key_avail,
        }
    }

    pub fn modifier_id(&self) -> usize
    {
        match self.val.0 {
            Keysym::Shift_L => MOD_SHIFT,
            Keysym::Control_L => MOD_CTRL,
            Keysym::Alt_L => MOD_ALT,
            Keysym::XF86_Fn => MOD_FN,
            Keysym::Meta_L => MOD_META,
            _ => 0,
        }
    }

    pub fn update_modifiers<K: Keyboard>(&mut self,
                                         kbd: &K,
                                         mod_state: &[ModState],
                                         text_avail: bool)
    {
        let mut val = self.orig.clone();

        if mod_state[MOD_FN - 1] != ModState::Released {
            val = Self::modify_fn(&val);
        } else {
            val = Self::modify_no_fn(&val);
        }

        if mod_state[MOD_SHIFT - 1] != ModState::Released {
            val = Self::modify_shift_label(&val);
        }

        let key_supp = kbd.key_supported(val.0);
        self.key_avail = val.0 != Keysym::NoSymbol && key_supp;
        self.text_avail = text_avail && Self::keyvalue_has_text(&val);
        self.val = val;
    }

    pub fn display_label(&self) -> &str
    {
        match self.val.1.as_ref() {
            " " => "␣",
            "\t" => "\\t",
            l => l,
        }
    }
}

#[derive(Clone)]
pub struct Key {
    pub parts: [Part; 9],
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

impl Key {
    fn new(x1: f64, y1: f64, x2: f64, y2: f64,
           center: KeyValue,
           top_left: KeyValue,
           top_right: KeyValue,
           bottom_left: KeyValue,
           bottom_right: KeyValue,
           left: KeyValue,
           right: KeyValue,
           top: KeyValue,
           bottom: KeyValue) -> Key
    {
        Key {
            x1, y1, x2, y2,
            parts: [
                Part::new(center),
                Part::new(top_left),
                Part::new(top_right),
                Part::new(bottom_left),
                Part::new(bottom_right),
                Part::new(left),
                Part::new(right),
                Part::new(top),
                Part::new(bottom),
            ],
        }
    }
}

const KEYSYMS: [(&str, Keysym, &str); 27] = [
    ("\\#", Keysym::numbersign, "#"),
    ("\\?", Keysym::question, "?"),
    ("\\@", Keysym::at, "@"),
    ("\\\\", Keysym::backslash, "\\"),
    ("alt", Keysym::Alt_L, "Alt"),
    ("backspace", Keysym::BackSpace, "⌫"),
    ("ctrl", Keysym::Control_L, "Ctrl"),
    ("delete", Keysym::Delete, "⌦"),
    ("down", Keysym::Down, "↓"),
    ("enter", Keysym::Return, "⏎"),
    ("esc", Keysym::Escape, "Esc"),
    ("f11_placeholder", Keysym::F11, "F11"),
    ("f12_placeholder", Keysym::F12, "F12"),
    ("fn", Keysym::XF86_Fn, "Fn"),
    ("left", Keysym::Left, "←"),
    ("meta", Keysym::Meta_L, "Meta"),
    ("print", Keysym::Print, "PrtScn"),
    ("right", Keysym::Right, "→"),
    ("shift", Keysym::Shift_L, "⇧"),
    ("space", Keysym::space, " "),
    ("switch_back_emoji", Keysym::Menu, "⌂"),
    ("switch_emoji", Keysym::Menu, "🙂"),
    ("switch_greekmath", Keysym::Scroll_Lock, "πλ∇¬"),
    ("switch_numeric", Keysym::Num_Lock, "123+"),
    ("switch_text", Keysym::Caps_Lock, "ABC"),
    ("tab", Keysym::Tab, "⭾"),
    ("up", Keysym::Up, "↑"),
];

#[derive(Clone)]
struct LayoutFile {
    rows: Vec<Vec<Key>>,
    has_bottom_row: bool,
    width: f64,
    height: f64,
}

struct LayoutLoader<'a> {
    file: LayoutFile,
    extra_keys: &'a Vec<String>,
    row_width: f64,
    in_row: u32,
}

impl<'a> LayoutLoader<'a> {
    fn get_keysym_by_name(extra_keys: &[String], name: &str) -> KeyValue
    {
        let name = if let Some(n) = name.strip_prefix("loc ") {
            if extra_keys.binary_search_by_key(&n, |k| k).is_ok() {
                n
            } else {
                return KeyValue::from(Keysym::NoSymbol, "")
            }
        } else {
            name
        };

        if let Ok(idx) = KEYSYMS.binary_search_by_key(&name, |&(k, _, _)| k) {
            return KeyValue::from(KEYSYMS[idx].1, KEYSYMS[idx].2);
        }

        let mut chars = name.chars();

        let c1 = chars.next();
        let c2 = chars.next();
        match (c1, c2) {
            (Some(c), None) => KeyValue::from(Keysym::from_char(c), name),
            _ => KeyValue::from(Keysym::NoSymbol, ""),
        }
    }

    fn start_key(&mut self, attrs: &HashMap<&str, &str>)
    {
        if self.in_row != 1 {
            println!("layout: key element not in row element ignored");
            return;
        }

        let shift = f64::from_str(attrs.get("shift").unwrap_or(&"")).unwrap_or(0.0);
        let x1 = self.row_width + shift;

        let width = f64::from_str(attrs.get("width").unwrap_or(&"")).unwrap_or(1.0);
        let x2 = x1 + width;

        self.row_width = x2;

        let key = Key::new(
            x1, self.file.height, x2, self.file.height + 1.0,
            Self::get_keysym_by_name(self.extra_keys, attrs.get("key0").unwrap_or(&"")),
            Self::get_keysym_by_name(self.extra_keys, attrs.get("key1").unwrap_or(&"")),
            Self::get_keysym_by_name(self.extra_keys, attrs.get("key2").unwrap_or(&"")),
            Self::get_keysym_by_name(self.extra_keys, attrs.get("key3").unwrap_or(&"")),
            Self::get_keysym_by_name(self.extra_keys, attrs.get("key4").unwrap_or(&"")),
            Self::get_keysym_by_name(self.extra_keys, attrs.get("key5").unwrap_or(&"")),
            Self::get_keysym_by_name(self.extra_keys, attrs.get("key6").unwrap_or(&"")),
            Self::get_keysym_by_name(self.extra_keys, attrs.get("key7").unwrap_or(&"")),
            Self::get_keysym_by_name(self.extra_keys, attrs.get("key8").unwrap_or(&""))
        );

        let idx = self.file.rows.len() - 1;
        self.file.rows[idx].push(key);
    }

    fn start_row(&mut self)
    {
        self.file.rows.push(Vec::new());
        self.in_row += 1;
        self.row_width = 0.0;
    }

    fn start_keyboard(&mut self, attrs: &HashMap<&str, &str>)
    {
        if *attrs.get("bottom_row").unwrap_or(&"") == "false" {
            self.file.has_bottom_row = false;
        }
    }

    fn start_elem(&mut self, name: &str, attrs: &HashMap<&str, &str>)
    {
        if name == "key" {
            self.start_key(attrs);
        } else if name == "row" {
            self.start_row();
        } else if name == "keyboard" {
            self.start_keyboard(attrs);
        }
    }

    fn end_row(&mut self)
    {
        self.in_row -= 1;

        if self.row_width > self.file.width {
            self.file.width = self.row_width;
        }

        self.file.height += 1.0;
    }

    fn end_elem(&mut self, name: &str)
    {
        if name == "row" {
            self.end_row();
        }
    }

    fn load<P: AsRef<Path>>(path: P, extra_keys: &'a Vec<String>) -> Result<LayoutFile, LayoutError>
    {
        let mut layout = LayoutLoader {
            file: LayoutFile {
                rows: Vec::new(),
                has_bottom_row: true,
                width: 0.0,
                height: 0.0,
            },
            extra_keys,
            row_width: 0.0,
            in_row: 0,
        };

        let mut vec = Vec::new();
        let mut file = File::open(&path)
            .map_err(|e| LayoutError::CannotOpen(path.as_ref().to_path_buf(), e))?;
        file.read_to_end(&mut vec)
            .map_err(|e| LayoutError::CannotRead(path.as_ref().to_path_buf(), e))?;

        let mut parser = ExpatParser::create()?;
        unsafe {
            expat::XML_SetElementHandler(parser.0, Some(start_elem), Some(end_elem));
            expat::XML_SetUserData(parser.0, &mut layout as *mut Self as *mut c_void);
        }
        parser.parse(path, &vec)?;

        Ok(layout.file)
    }
}

pub struct Layout {
    main_layout: LayoutFile,
    numeric: LayoutFile,
    greekmath: LayoutFile,
    emoji: LayoutFile,
    bottom_row: LayoutFile,

    rows: Vec<Vec<Key>>,
    width: f64,
    height: f64,
    text_supp: bool,
    emoji_mode: bool,
}

impl Layout {
    pub fn is_keysym_modifier(modifier: usize) -> bool
    {
        match modifier {
            MOD_FN => true,
            _ => false,
        }
    }

    pub fn is_pressable_modifier(modifier: usize) -> bool
    {
        !Self::is_keysym_modifier(modifier) && modifier != MOD_META
    }

    pub fn is_label_modifier(modifier: usize) -> bool
    {
        Self::is_keysym_modifier(modifier) || modifier == MOD_SHIFT
    }

    pub fn modifier_keysym(modifier: usize) -> Keysym
    {
        match modifier {
            MOD_SHIFT => Keysym::Shift_L,
            MOD_CTRL => Keysym::Control_L,
            MOD_ALT => Keysym::Alt_L,
            MOD_FN => Keysym::XF86_Fn,
            MOD_META => Keysym::Meta_L,
            _ => Keysym::NoSymbol,
        }
    }

    fn open_layout(&mut self, data: LayoutFile)
    {
        self.rows = data.rows;
        self.width = data.width;
        self.height = data.height;

        if data.has_bottom_row {
            let old_height = self.height;
            self.height += self.bottom_row.height;

            self.rows.reserve(self.bottom_row.rows.len());
            for row in &self.bottom_row.rows {
                let mut new_row = Vec::with_capacity(row.len());

                for key in row {
                    new_row.push(Key {
                        parts: key.parts.clone(),
                        x1: key.x1 * self.width / self.bottom_row.width,
                        y1: old_height + key.y1,
                        x2: key.x2 * self.width / self.bottom_row.width,
                        y2: old_height + key.y2,
                    });
                }

                self.rows.push(new_row);
            }
        }
    }

    pub fn load(cfg: &Configuration) -> Result<Self, LayoutError>
    {
        let extra_keys = cfg.extra_keys();
        let base_dirs = BaseDirectories::with_prefix("unfettered-keyboard/layouts");

        let path = base_dirs.find_data_file(cfg.layout())
            .ok_or(LayoutError::NotFound(cfg.layout().to_string()))?;
        let main_layout = LayoutLoader::load(path, extra_keys)?;

        let path = base_dirs.find_data_file("numeric.xml")
            .ok_or(LayoutError::NotFound(String::from("numeric.xml")))?;
        let numeric = LayoutLoader::load(path, extra_keys)?;

        let path = base_dirs.find_data_file("greekmath.xml")
            .ok_or(LayoutError::NotFound(String::from("greekmath.xml")))?;
        let greekmath = LayoutLoader::load(path, extra_keys)?;

        let path = base_dirs.find_data_file("emoji.xml")
            .ok_or(LayoutError::NotFound(String::from("emoji.xml")))?;
        let emoji = LayoutLoader::load(path, extra_keys)?;

        let path = base_dirs.find_data_file("bottom_row.xml")
            .ok_or(LayoutError::NotFound(String::from("bottom_row.xml")))?;
        let bottom_row = LayoutLoader::load(path, extra_keys)?;

        let mut layout = Layout {
            main_layout,
            numeric,
            greekmath,
            emoji,
            bottom_row,

            rows: Vec::new(),
            width: 0.0,
            height: 0.0,
            text_supp: false,
            emoji_mode: false,
        };

        let data = layout.main_layout.clone();
        layout.open_layout(data);

        Ok(layout)
    }

    #[inline(always)]
    pub fn width(&self) -> f64
    {
        self.width
    }

    #[inline(always)]
    pub fn height(&self) -> f64
    {
        self.height
    }

    #[inline(always)]
    pub fn rows(&self) -> &Vec<Vec<Key>>
    {
        &self.rows
    }

    pub fn locate_key(&self, x: f64, y: f64) -> Option<&Key>
    {
        let row = &self.rows[y as usize];
        let res = row.binary_search_by(|k| {
            if k.x2 < x {
                Ordering::Less
            } else if k.x1 > x {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        });

        match res {
            Ok(i) => Some(&row[i]),
            Err(_) => None,
        }
    }

    pub fn locate_key_mut(&mut self, x: f64, y: f64) -> Option<&mut Key>
    {
        let row = &mut self.rows[y as usize];
        let res = row.binary_search_by(|k| {
            if k.x2 < x {
                Ordering::Less
            } else if k.x1 > x {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        });

        match res {
            Ok(i) => Some(&mut row[i]),
            Err(_) => None,
        }
    }

    pub fn update_modifiers<K: Keyboard>(&mut self, kbd: &K, mod_state: &[ModState])
    {
        for row in self.rows.iter_mut() {
            for key in row.iter_mut() {
                for part in key.parts.iter_mut() {
                    part.update_modifiers(kbd, mod_state, self.text_supp);
                }
            }
        }
    }

    pub fn set_text_supported(&mut self, text_supp: bool)
    {
        self.text_supp = text_supp;

        for row in self.rows.iter_mut() {
            for key in row.iter_mut() {
                for part in key.parts.iter_mut() {
                    part.set_text_supported(text_supp);
                }
            }
        }
    }

    pub fn update_keys_supported<K: Keyboard>(&mut self, kbd: &K)
    {
        for row in self.rows.iter_mut() {
            for key in row.iter_mut() {
                for part in key.parts.iter_mut() {
                    part.update_keys_supported(kbd);
                }
            }
        }
    }

    pub fn switch_numeric(&mut self)
    {
        self.emoji_mode = false;
        let data = self.numeric.clone();
        self.open_layout(data);
        self.set_text_supported(self.text_supp);
    }

    pub fn switch_greekmath(&mut self)
    {
        self.emoji_mode = false;
        let data = self.greekmath.clone();
        self.open_layout(data);
        self.set_text_supported(self.text_supp);
    }

    pub fn switch_emoji(&mut self)
    {
        if self.emoji_mode {
            self.emoji_mode = false;
            let data = self.main_layout.clone();
            self.open_layout(data);
        } else {
            self.emoji_mode = true;
            let data = self.emoji.clone();
            self.open_layout(data);
        }

        self.set_text_supported(self.text_supp);
    }

    pub fn switch_text(&mut self)
    {
        self.emoji_mode = false;
        let data = self.main_layout.clone();
        self.open_layout(data);
        self.set_text_supported(self.text_supp);
    }
}
