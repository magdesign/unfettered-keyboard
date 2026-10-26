// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2025, Unfettered Keyboard Contributors
 */

use crate::core::Configuration;
use crate::core::Part;
use linux_raw_sys::ioctl::KDSKBENT;
use linux_raw_sys::ioctl::KDSKBSENT;
use rustix::termios;
use std::fs::File;
use std::io::Error;
use std::os::fd::AsRawFd;
use xkeysym::Keysym;

#[repr(C)]
struct KbEntry {
    table: u8,
    index: u8,
    value: u16,
}

#[repr(C)]
struct KbSEntry {
    func: u8,
    string: [u8; 512],
}

pub struct Teletype {
    f: File,
    orig_ws: termios::Winsize,
}

const KT_LATIN: u8 = 0;
const KT_FN: u8 = 1;
const KT_SPEC: u8 = 2;
const KT_CUR: u8 = 6;
const KT_SHIFT: u8 = 7;

fn key(kt: u8, kv: u8) -> u16
{
    ((kt as u16) << 8) | (kv as u16)
}

impl Teletype {
    pub fn keysym_value(sym: Keysym) -> Option<u16>
    {
        if sym.raw() >= 0x20 && sym.raw() <= 0x7e {
            return Some(sym.raw() as u16);
        }

        match sym {
            Keysym::BackSpace => Some(key(KT_LATIN, 0x7f)),
            Keysym::Tab => Some(key(KT_LATIN, 0x09)),
            Keysym::Return => Some(key(KT_SPEC, 0x01)),
            Keysym::Escape => Some(key(KT_LATIN, 0x1b)),
            Keysym::Home => Some(key(KT_FN, 0x14)),
            Keysym::Left => Some(key(KT_CUR, 0x01)),
            Keysym::Up => Some(key(KT_CUR, 0x03)),
            Keysym::Right => Some(key(KT_CUR, 0x02)),
            Keysym::Down => Some(key(KT_CUR, 0x00)),
            Keysym::Prior => Some(key(KT_FN, 0x18)),
            Keysym::Next => Some(key(KT_FN, 0x19)),
            Keysym::End => Some(key(KT_FN, 0x18)),
            Keysym::Insert => Some(key(KT_FN, 0x15)),
            Keysym::F1 => Some(key(KT_FN, 0x00)),
            Keysym::F2 => Some(key(KT_FN, 0x01)),
            Keysym::F3 => Some(key(KT_FN, 0x02)),
            Keysym::F4 => Some(key(KT_FN, 0x03)),
            Keysym::F5 => Some(key(KT_FN, 0x04)),
            Keysym::F6 => Some(key(KT_FN, 0x05)),
            Keysym::F7 => Some(key(KT_FN, 0x06)),
            Keysym::F8 => Some(key(KT_FN, 0x07)),
            Keysym::F9 => Some(key(KT_FN, 0x08)),
            Keysym::F10 => Some(key(KT_FN, 0x09)),
            Keysym::F11 => Some(key(KT_FN, 0x0a)),
            Keysym::F12 => Some(key(KT_FN, 0x0b)),
            Keysym::Shift_L => Some(key(KT_SHIFT, 0x00)),
            Keysym::Control_L => Some(key(KT_SHIFT, 0x02)),
            Keysym::Alt_L => Some(key(KT_SHIFT, 0x03)),
            Keysym::Delete => Some(key(KT_FN, 0x16)),
            _ => None,
        }
    }

    pub fn new(cfg: &Configuration) -> Result<Self, Error>
    {
        let f = File::open("/dev/tty0")?;

        let orig_ws = termios::tcgetwinsize(&f)?;

        let mut ws = orig_ws;
        ws.ws_row = (ws.ws_row as f64 - cfg.evfb_height() * ws.ws_row as f64) as u16;
        termios::tcsetwinsize(&f, ws)?;

        Ok(Self { f, orig_ws })
    }

    fn write_entry(&self, table: u8, index: u8, value: u16) -> Result<(), Error>
    {
        let ent = KbEntry { table, index, value };
        let ret = unsafe { libc::ioctl(self.f.as_raw_fd(),
                                       KDSKBENT as libc::Ioctl,
                                       &ent as *const _) };

        if ret == -1 {
            Err(Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub fn write_key(&self, part: &Part, keycode: u8) -> Result<(), Error>
    {
        let lower = part.sym();
        let upper = Part::modify_shift(lower);

        let val = Self::keysym_value(lower).unwrap();
        self.write_entry(0, keycode, val)?;

        let val = Self::keysym_value(upper).unwrap();
        self.write_entry(1, keycode, val)?;

        Ok(())
    }

    pub fn write_text_key(&self, keycode: u8) -> Result<(), Error>
    {
        self.write_entry(0, keycode, key(KT_FN, 254))?;
        self.write_entry(1, keycode, key(KT_FN, 254))?;
        Ok(())
    }

    pub fn write_text(&self, text: &str) -> Result<(), Error>
    {
        let text = text.as_bytes();

        let mut ent = KbSEntry { func: 254, string: [0; 512] };
        let dest = &mut ent.string[..text.len()];
        dest.copy_from_slice(text);
        ent.string[511] = 0;

        let ret = unsafe { libc::ioctl(self.f.as_raw_fd(),
                                       KDSKBSENT as libc::Ioctl,
                                       &ent as *const _) };
        if ret == -1 {
            Err(Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl Drop for Teletype {
    fn drop(&mut self)
    {
        let _ = termios::tcsetwinsize(&self.f, self.orig_ws);
    }
}
