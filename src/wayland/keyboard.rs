// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

use crate::core::Keyboard;
use crate::core::Layout;
use crate::core::Part;
use crate::evdev::STATIC_KEYCODES;
use crate::wayland::Dispatcher;
use crate::wayland::input_method_unstable_v1::zwp_input_method_context_v1::ZwpInputMethodContextV1;
use crate::wayland::input_method_unstable_v2::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use crate::wayland::input_method_unstable_v2::zwp_input_method_v2::ZwpInputMethodV2;
use crate::wayland::virtual_keyboard_unstable_v1::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use crate::wayland::virtual_keyboard_unstable_v1::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;
use reis::ei;
use std::collections::HashMap;
use std::fs;
use std::fmt::Write as _;
use std::io::Error;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::fd::FromRawFd;
use wayland_client::QueueHandle;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_keyboard::KeymapFormat;
use xkeysym::Keysym;

pub struct VirtualKeyboard {
    vk: Option<ZwpVirtualKeyboardV1>,
    im: Option<ZwpInputMethodV2>,
    im_ctx: Option<ZwpInputMethodContextV1>,
    ei_kbd: Option<(ei::device::Device, ei::keyboard::Keyboard)>,
    im_serial: u32,
    ei_serial: u32,

    keycodes: HashMap<Keysym, u16>,
    pressed: [Keysym; 248],
    mod_state: u32,
}

impl VirtualKeyboard {
    pub fn new(queue: &QueueHandle<Dispatcher>,
               vk_man: &Option<ZwpVirtualKeyboardManagerV1>,
               im_man: &Option<ZwpInputMethodManagerV2>,
               seat: &WlSeat)
            -> VirtualKeyboard
    {
        let vk = vk_man.as_ref().map(|m| m.create_virtual_keyboard(seat, queue, ()));
        let im = im_man.as_ref().map(|m| m.get_input_method(seat, queue, ()));

        VirtualKeyboard {
            vk,
            im,
            im_ctx: None,
            ei_kbd: None,
            im_serial: 0,
            ei_serial: 0,

            keycodes: HashMap::with_capacity(248),
            pressed: [Keysym::NoSymbol; 248],
            mod_state: 0,
        }
    }

    /*
     * Append the symbols of a key to the keymap. Returns false if the keysym
     * has no name that xkbcommon could parse.
     */
    fn write_key(keymap: &mut String, sym: Keysym, keycode: u16) -> bool
    {
        let lower = match sym.name().and_then(|n| n.strip_prefix("XK_")) {
            Some(n) => n,
            None => return false,
        };
        let upper = Part::modify_shift(sym).name()
                        .and_then(|n| n.strip_prefix("XK_"))
                        .unwrap_or(lower);

        // Writing to a String cannot fail.
        let _ = writeln!(keymap, "    key <I{:03}> {{ [ {}, {} ] }};",
                         keycode, lower, upper);

        let modifier = match sym {
            Keysym::Shift_L => Some("Shift"),
            Keysym::Control_L => Some("Control"),
            Keysym::Alt_L => Some("Mod1"),
            _ => None,
        };

        if let Some(modifier) = modifier {
            let _ = writeln!(keymap, "    modifier_map {} {{ <I{:03}> }};",
                             modifier, keycode);
        }

        true
    }

    /*
     * Upload a keymap through an anonymous memory file. The keymap is sent
     * NUL-terminated, as compositors parse it as a C string.
     */
    fn keymap_file(keymap: &str) -> Result<fs::File, Error>
    {
        let name = b"ufkbd-keymap\0";
        let fd = unsafe {
            libc::memfd_create(name.as_ptr() as *const libc::c_char, libc::MFD_CLOEXEC)
        };
        if fd == -1 {
            return Err(Error::last_os_error());
        }

        let mut file = unsafe { fs::File::from_raw_fd(fd) };
        file.write_all(keymap.as_bytes())?;
        file.write_all(b"\0")?;

        Ok(file)
    }

    pub fn done(&mut self)
    {
        self.im_serial += 1;
    }

    pub fn set_im_serial(&mut self, serial: u32)
    {
        self.im_serial = serial;
    }

    pub fn set_ei_serial(&mut self, serial: u32)
    {
        self.ei_serial = serial;
    }

    pub fn set_input_method_context(&mut self, imc: Option<ZwpInputMethodContextV1>)
    {
        self.im_ctx = imc;
    }

    pub fn set_ei_keyboard(&mut self,
                           dev: ei::device::Device,
                           kbd: ei::keyboard::Keyboard)
    {
        self.ei_kbd = Some((dev, kbd));
    }
}

impl Keyboard for VirtualKeyboard {
    fn key_supported(&self, sym: Keysym) -> bool
    {
        if self.vk.is_some() {
            match sym.name() {
                Some(n) => n.starts_with("XK_"),
                None => false,
            }
        } else if self.ei_kbd.is_some() {
            STATIC_KEYCODES.binary_search_by_key(&sym, |(s, _)| *s).is_ok()
        } else {
            self.im_ctx.is_some()
        }
    }

    fn press(&mut self, sym: Keysym)
    {
        match sym {
            Keysym::Shift_L => {
                self.mod_state |= 0x1;
            },
            Keysym::Control_L => {
                self.mod_state |= 0x4;
            },
            Keysym::Alt_L => {
                self.mod_state |= 0x8;
            },
            _ => (),
        }

        if let Some(vk) = &self.vk {
            if sym == Keysym::Shift_L
            || sym == Keysym::Control_L
            || sym == Keysym::Alt_L {
                vk.modifiers(self.mod_state, 0, 0, 0);
            }

            let keycode = match self.keycodes.get(&sym) {
                Some(&k) => k,
                None => {
                    eprintln!("warn: {:?} is not in the keymap, not pressing it", sym);
                    return;
                },
            };
            self.pressed[keycode as usize] = sym;
            vk.key(0, keycode as u32, 1);
        } else if let Some((d, k)) = &self.ei_kbd {
            let keycode = match STATIC_KEYCODES.binary_search_by_key(&sym, |(s, _)| *s) {
                Ok(i) => STATIC_KEYCODES[i].1.0,
                Err(_) => return,
            };
            k.key(keycode as u32, ei::keyboard::KeyState::Press);
            d.frame(self.ei_serial, 0);
        } else if let Some(imc) = &self.im_ctx {
            imc.keysym(self.im_serial, 0, sym.raw(), 0, self.mod_state);
        }
    }

    fn release(&mut self, sym: Keysym)
    {
        match sym {
            Keysym::Shift_L => {
                self.mod_state &= !0x1;
            },
            Keysym::Control_L => {
                self.mod_state &= !0x4;
            },
            Keysym::Alt_L => {
                self.mod_state &= !0x8;
            },
            _ => (),
        }

        if let Some(vk) = &self.vk {
            if sym == Keysym::Shift_L
            || sym == Keysym::Control_L
            || sym == Keysym::Alt_L {
                vk.modifiers(self.mod_state, 0, 0, 0);
            }

            let keycode = match self.keycodes.get(&sym) {
                Some(&k) => k,
                None => {
                    eprintln!("warn: {:?} is not in the keymap, not releasing it", sym);
                    return;
                },
            };
            self.pressed[keycode as usize] = Keysym::NoSymbol;
            vk.key(0, keycode as u32, 0);
        } else if let Some((d, k)) = &self.ei_kbd {
            let keycode = match STATIC_KEYCODES.binary_search_by_key(&sym, |(s, _)| *s) {
                Ok(i) => STATIC_KEYCODES[i].1.0,
                Err(_) => return,
            };
            k.key(keycode as u32, ei::keyboard::KeyState::Released);
            d.frame(self.ei_serial, 0);
        } else if let Some(imc) = &self.im_ctx {
            imc.keysym(self.im_serial, 0, sym.raw(), 0, self.mod_state);
        }
    }

    fn text(&mut self, text: &str)
    {
        if let Some(im) = &self.im {
            im.commit_string(text.to_string());
            im.commit(self.im_serial);
        } else if let Some(imc) = &self.im_ctx {
            imc.commit_string(self.im_serial, text.to_string());
        } else {
            panic!("Text should not be emitted without an input method protocol");
        }
    }

    fn change_layout(&mut self, layout: &Layout)
    {
        // Only the virtual keyboard protocol uses a keymap.
        let vk = match &self.vk {
            Some(vk) => vk.clone(),
            None => return,
        };

        /*
         * Keycodes are evdev codes; the keymap names them with the XKB keycode
         * (evdev code + 8), which must not exceed 255.
         */
        const CODES_MAX: usize = 248;

        let mut keycodes: HashMap<Keysym, u16> = HashMap::with_capacity(CODES_MAX);
        let mut used_codes = [false; CODES_MAX];
        let mut symbols = String::new();

        let static_code = |sym: Keysym| {
            STATIC_KEYCODES.binary_search_by_key(&sym, |(s, _)| *s)
                           .ok()
                           .map(|idx| STATIC_KEYCODES[idx].1.0)
        };

        /*
         * Modifiers are always in the keymap, even if the layout has no key
         * for them, because a latched or locked modifier stays pressed while
         * switching to a layout without it (e.g. emoji).
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

        // First pass: keys with a well-known evdev code.
        for &sym in &wanted {
            if keycodes.contains_key(&sym) {
                continue;
            }

            if let Some(code) = static_code(sym) {
                if (code as usize) < CODES_MAX && !used_codes[code as usize]
                && Self::write_key(&mut symbols, sym, code + 8) {
                    keycodes.insert(sym, code);
                    used_codes[code as usize] = true;
                }
            }
        }

        // Second pass: everything else gets a free code.
        let mut next_code = 0;
        for &sym in &wanted {
            if keycodes.contains_key(&sym) {
                continue;
            }

            while next_code < CODES_MAX && used_codes[next_code] {
                next_code += 1;
            }

            if next_code >= CODES_MAX {
                eprintln!("warn: layout has too many keys for a keymap, some keys will not work");
                break;
            }

            if Self::write_key(&mut symbols, sym, next_code as u16 + 8) {
                keycodes.insert(sym, next_code as u16);
                used_codes[next_code] = true;
                next_code += 1;
            }
        }

        let mut keymap = String::new();
        keymap.push_str("xkb_keymap {\n");
        keymap.push_str("  xkb_symbols \"ufkbd\" {\n");
        keymap.push_str(&symbols);
        keymap.push_str("  };\n");
        keymap.push_str("\n");
        keymap.push_str("  xkb_keycodes \"ufkbd\" {\n");
        keymap.push_str("    minimum = 8;\n");
        keymap.push_str("    maximum = 255;\n");

        for (code, used) in used_codes.iter().enumerate() {
            if *used {
                let _ = writeln!(keymap, "    <I{:03}> = {};", code + 8, code + 8);
            }
        }

        keymap.push_str("    indicator 1 = \"Caps Lock\";\n");
        keymap.push_str("  };\n");
        keymap.push_str("\n");
        keymap.push_str("  xkb_types \"ufkbd\" {\n");
        keymap.push_str("    type \"TWO_LEVEL\" {\n");
        keymap.push_str("        modifiers = Shift;\n");
        keymap.push_str("        map[Shift] = Level2;\n");
        keymap.push_str("        level_name[Level1] = \"Base\";\n");
        keymap.push_str("        level_name[Level2] = \"Shift\";\n");
        keymap.push_str("    };\n");
        keymap.push_str("  };\n");
        keymap.push_str("\n");
        keymap.push_str("  xkb_compatibility \"ufkbd\" {\n");
        keymap.push_str("  };\n");
        keymap.push_str("};\n");

        let file = match Self::keymap_file(&keymap) {
            Ok(f) => f,
            Err(e) => {
                // Keep the old keymap; keys missing from it are skipped.
                eprintln!("warn: failed to create keymap: {}", e);
                return;
            },
        };
        // Including the terminating NUL; a keymap is far smaller than 4 GiB.
        let len = (keymap.len() + 1) as u32;

        // Release everything under the old keymap, whose codes may change.
        if self.mod_state != 0 {
            vk.modifiers(0, 0, 0, 0);
        }

        let mut held = Vec::new();
        for (code, sym) in self.pressed.iter_mut().enumerate() {
            if *sym != Keysym::NoSymbol {
                vk.key(0, code as u32, 0);
                held.push(*sym);
                *sym = Keysym::NoSymbol;
            }
        }

        vk.keymap(KeymapFormat::XkbV1 as u32, file.as_fd(), len);
        self.keycodes = keycodes;

        if self.mod_state != 0 {
            vk.modifiers(self.mod_state, 0, 0, 0);
        }

        // Press the held keys again with their new codes.
        for sym in held {
            match self.keycodes.get(&sym) {
                Some(&code) => {
                    self.pressed[code as usize] = sym;
                    vk.key(0, code as u32, 1);
                },
                None => eprintln!("warn: {:?} is not in the new keymap, releasing it", sym),
            }
        }
    }
}
