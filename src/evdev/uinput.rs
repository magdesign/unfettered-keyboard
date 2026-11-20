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
        self.emit_key(sym, 1);
    }

    fn release(&mut self, sym: Keysym)
    {
        self.emit_key(sym, 0);
    }

    fn text(&mut self, text: &str)
    {
        if let Err(e) = self.tty.write_text(text) {
            eprintln!("warn: failed to set text {:?}: {}", text, e);
            return;
        }

        self.press(Keysym::Shift_R);
        self.release(Keysym::Shift_R);
    }

    fn change_layout(&mut self, layout: &Layout)
    {
        // Key code 0 is KEY_RESERVED and is never emitted.
        let mut used_codes = [false; 256];
        used_codes[0] = true;

        /*
         * Modifiers are always mapped, even if the layout has no key for them,
         * because a latched or locked modifier stays pressed while switching
         * to a layout without it (e.g. emoji).
         */
        let mut wanted = vec![Keysym::Shift_L, Keysym::Control_L, Keysym::Alt_L];
        for row in layout.rows() {
            for key in row {
                for part in &key.parts {
                    if part.key_available() {
                        wanted.push(part.sym());
                    }
                }
            }
        }

        self.keycodes.clear();

        // First pass: keys with a well-known key code.
        for &sym in &wanted {
            if self.keycodes.contains_key(&sym) {
                continue;
            }

            if let Ok(idx) = STATIC_KEYCODES.binary_search_by_key(&sym, |(s, _)| *s) {
                let code = STATIC_KEYCODES[idx].1.0;
                if code == 0 || code > 255 || used_codes[code as usize] {
                    continue;
                }

                match self.tty.write_key(sym, code as u8) {
                    Ok(()) => {
                        self.keycodes.insert(sym, code as u8);
                        used_codes[code as usize] = true;
                    },
                    Err(e) => eprintln!("warn: failed to map {:?}: {}", sym, e),
                }
            }
        }

        // Second pass: everything else, plus the text key, gets a free code.
        let mut next_code = 1;
        let others = wanted.iter().copied().chain(std::iter::once(Keysym::Shift_R));
        for sym in others {
            if self.keycodes.contains_key(&sym) {
                continue;
            }

            while next_code < used_codes.len() && used_codes[next_code] {
                next_code += 1;
            }

            if next_code >= used_codes.len() {
                eprintln!("warn: layout has too many keys for the console, some keys will not work");
                break;
            }

            let res = if sym == Keysym::Shift_R {
                self.tty.write_text_key(next_code as u8)
            } else {
                self.tty.write_key(sym, next_code as u8)
            };

            match res {
                Ok(()) => {
                    self.keycodes.insert(sym, next_code as u8);
                    used_codes[next_code] = true;
                    next_code += 1;
                },
                Err(e) => eprintln!("warn: failed to map {:?}: {}", sym, e),
            }
        }
    }
}

impl VirtualKeyboard {
    fn emit_key(&mut self, sym: Keysym, value: i32)
    {
        let code = match self.keycodes.get(&sym) {
            Some(&c) => c,
            None => {
                eprintln!("warn: {:?} is not mapped to a key code", sym);
                return;
            },
        };

        let evt = InputEvent::new_now(EventType::KEY.0, code as u16, value);
        if let Err(e) = self.dev.emit(&[evt]) {
            eprintln!("warn: failed to emit {:?}: {}", sym, e);
        }
    }
}
