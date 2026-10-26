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
use evdev::KeyCode;
use reis::ei;
use std::collections::HashMap;
use std::fs;
use std::io::Seek;
use std::io::Write;
use std::os::fd::AsFd;
use std::process;
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

    fn write_key(keymap: &mut fs::File, part: &Part, keycode: u16)
    {
        let sym = part.sym();
        let lower = &sym.name().unwrap()[3..];
        let upper = &Part::modify_shift(sym).name().unwrap()[3..];

        writeln!(keymap, "    key <I{:03}> {{ [ {}, {} ] }};",
                 keycode, lower, upper).unwrap();

        if sym == Keysym::Shift_L {
            writeln!(keymap,
                     "    modifier_map Shift {{ <I{:03}> }};",
                     keycode).unwrap();
        } else if sym == Keysym::Control_L {
            writeln!(keymap,
                     "    modifier_map Control {{ <I{:03}> }};",
                     keycode).unwrap();
        } else if sym == Keysym::Alt_L {
            writeln!(keymap,
                     "    modifier_map Mod1 {{ <I{:03}> }};",
                     keycode).unwrap();
        }
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

            let keycode = *self.keycodes.get(&sym).unwrap();
            self.pressed[keycode as usize] = sym;
            vk.key(0, keycode as u32, 1);
        } else if let Some((d, k)) = &self.ei_kbd {
            let i = STATIC_KEYCODES.binary_search_by_key(&sym, |(s, _)| *s).unwrap();
            let keycode = STATIC_KEYCODES[i].1.0;
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

            let keycode = *self.keycodes.get(&sym).unwrap();
            self.pressed[keycode as usize] = Keysym::NoSymbol;
            vk.key(0, keycode as u32, 0);
        } else if let Some((d, k)) = &self.ei_kbd {
            let i = STATIC_KEYCODES.binary_search_by_key(&sym, |(s, _)| *s).unwrap();
            let keycode = STATIC_KEYCODES[i].1.0;
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
        let mut keycode = 8;
        let mut used_codes = [false; 248];

        let path = format!("/tmp/ufkbd-keymap-pid{}", process::id());
        let mut keymap = fs::File::create_new(path.clone()).unwrap();
        let _ = fs::remove_file(path);

        keymap.write_all(b"xkb_keymap {\n").unwrap();
        keymap.write_all(b"  xkb_symbols \"ufkbd\" {\n").unwrap();

        self.keycodes.clear();
        for row in layout.rows() {
            for key in row {
                for part in &key.parts {
                    if !part.key_available() {
                        continue;
                    }

                    if let Ok(idx) = STATIC_KEYCODES.binary_search_by_key(&part.sym(), |(s, _)| *s) {
                        let (sym, KeyCode(code)) = STATIC_KEYCODES[idx];
                        self.keycodes.insert(sym, code);
                        Self::write_key(&mut keymap, part, code + 8);
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

                    while used_codes[keycode as usize - 8] {
                        keycode += 1;
                    }

                    self.keycodes.insert(part.sym(), keycode - 8);
                    Self::write_key(&mut keymap, part, keycode);
                    keycode += 1;
                }
            }
        }

        keymap.write_all(b"  };\n").unwrap();
        keymap.write_all(b"\n").unwrap();
        keymap.write_all(b"  xkb_keycodes \"ufkbd\" {\n").unwrap();
        keymap.write_all(b"    minimum = 8;\n").unwrap();
        keymap.write_all(b"    maximum = 255;\n").unwrap();

        for i in 8..keycode {
            writeln!(keymap, "    <I{:03}> = {};", i, i).unwrap();
        }

        for i in keycode..255 {
            if used_codes[i as usize - 8] {
                writeln!(keymap, "    <I{:03}> = {};", i, i).unwrap();
            }
        }

        keymap.write_all(b"    indicator 1 = \"Caps Lock\";\n").unwrap();
        keymap.write_all(b"  };\n").unwrap();
        keymap.write_all(b"\n").unwrap();
        keymap.write_all(b"  xkb_types \"ufkbd\" {\n").unwrap();
        keymap.write_all(b"    type \"TWO_LEVEL\" {\n").unwrap();
        keymap.write_all(b"        modifiers = Shift;\n").unwrap();
        keymap.write_all(b"        map[Shift] = Level2;\n").unwrap();
        keymap.write_all(b"        level_name[Level1] = \"Base\";\n").unwrap();
        keymap.write_all(b"        level_name[Level2] = \"Shift\";\n").unwrap();
        keymap.write_all(b"    };\n").unwrap();
        keymap.write_all(b"  };\n").unwrap();
        keymap.write_all(b"\n").unwrap();
        keymap.write_all(b"  xkb_compatibility \"ufkbd\" {\n").unwrap();
        keymap.write_all(b"  };\n").unwrap();
        keymap.write_all(b"};\n").unwrap();

        if let Some(vk) = self.vk.as_ref() {
            if self.mod_state != 0 {
                vk.modifiers(0, 0, 0, 0);
            }

            for (code, sym) in self.pressed.iter().enumerate() {
                if *sym != Keysym::NoSymbol {
                    vk.key(0, code as u32, 0);
                }
            }

            let len = keymap.stream_position().unwrap() as u32;
            vk.keymap(KeymapFormat::XkbV1 as u32, keymap.as_fd(), len);

            if self.mod_state != 0 {
                vk.modifiers(self.mod_state, 0, 0, 0);
            }

            for sym in &self.pressed {
                if *sym != Keysym::NoSymbol {
                    let code = *self.keycodes.get(sym).unwrap();
                    vk.key(0, code as u32, 1);
                }
            }
        }
    }
}
