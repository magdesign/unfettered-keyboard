// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2025, Unfettered Keyboard Contributors
 */

use crate::core::Keyboard;
use crate::core::Layout;
use crate::evdev::Teletype;
use crate::evdev::STATIC_KEYCODES;
use evdev::uinput::VirtualDevice;
use evdev::AttributeSet;
use evdev::EventType;
use evdev::InputEvent;
use evdev::KeyCode;
use std::collections::HashMap;
use std::io::Error;
use std::iter::FromIterator;
use xkeysym::Keysym;

pub struct VirtualKeyboard {
    dev: VirtualDevice,
    tty: Teletype,
    keycodes: HashMap<Keysym, u8>,
}

impl VirtualKeyboard {
    pub fn new(tty: Teletype) -> Result<Self, Error>
    {
        let keycodes = (1..=255).into_iter().map(|c| KeyCode(c));
        let keycodes = AttributeSet::from_iter(keycodes);

        let builder = VirtualDevice::builder()?;
        let builder = builder.name("Unfettered Keyboard");
        let builder = builder.with_keys(&keycodes)?;
        let dev = builder.build()?;

        Ok(Self {
            dev,
            tty,
            keycodes: HashMap::with_capacity(256),
        })
    }
}

impl Keyboard for VirtualKeyboard {
    fn key_supported(&self, sym: Keysym) -> bool
    {
        Teletype::keysym_value(sym).is_some()
    }

    fn press(&mut self, sym: Keysym)
    {
        let code = *self.keycodes.get(&sym).unwrap();
        let evt = InputEvent::new_now(EventType::KEY.0, code as u16, 1);
        self.dev.emit(&[evt]).unwrap();
    }

    fn release(&mut self, sym: Keysym)
    {
        let code = *self.keycodes.get(&sym).unwrap();
        let evt = InputEvent::new_now(EventType::KEY.0, code as u16, 0);
        self.dev.emit(&[evt]).unwrap();
    }

    fn text(&mut self, text: &str)
    {
        self.tty.write_text(text).unwrap();
        self.press(Keysym::Shift_R);
        self.release(Keysym::Shift_R);
    }

    fn change_layout(&mut self, layout: &Layout)
    {
        let mut keycode = 1;
        let mut used_codes = [false; 256];

        self.keycodes.clear();
        for row in layout.rows() {
            for key in row {
                for part in &key.parts {
                    if !part.key_available() {
                        continue;
                    }

                    if let Ok(idx) = STATIC_KEYCODES.binary_search_by_key(&part.sym(), |(s, _)| *s) {
                        let (sym, KeyCode(code)) = STATIC_KEYCODES[idx];
                        self.keycodes.insert(sym, code as u8);
                        self.tty.write_key(part, code as u8).unwrap();
                        used_codes[code as usize] = true;
                    }
                }
            }
        }

        for row in layout.rows() {
            for key in row {
                for part in &key.parts {
                    if !part.key_available() {
                        continue;
                    }

                    if self.keycodes.contains_key(&part.sym()) {
                        continue;
                    }

                    while used_codes[keycode as usize] {
                        keycode += 1;
                    }

                    self.keycodes.insert(part.sym(), keycode);
                    self.tty.write_key(part, keycode).unwrap();
                    keycode += 1;
                }
            }
        }

        while used_codes[keycode as usize] {
            keycode += 1;
        }

        self.keycodes.insert(Keysym::Shift_R, keycode);
        self.tty.write_text_key(keycode).unwrap();
    }
}
