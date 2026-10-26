// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2025, Unfettered Keyboard Contributors
 */

use crate::core::Keyboard;
use crate::core::Layout;
use std::iter::FromIterator;
use xkeysym::Keysym;

pub struct Output {
    back: String,
    front: String,
    complete: bool,
}

impl Output {
    pub fn new() -> Self
    {
        Self {
            back: String::new(),
            front: String::new(),
            complete: false,
        }
    }

    #[inline(always)]
    pub fn complete(&self) -> bool
    {
        self.complete
    }
}

impl Keyboard for Output {
    fn key_supported(&self, sym: Keysym) -> bool
    {
        match sym {
            Keysym::Return => true,
            Keysym::BackSpace => true,
            Keysym::Delete => true,
            Keysym::Left => true,
            Keysym::Right => true,
            _ => false,
        }
    }

    fn press(&mut self, sym: Keysym)
    {
        match sym {
            Keysym::Return => {
                let front = self.front.chars().rev();
                let front = String::from_iter(front);

                println!("{}{}", self.back, front);

                self.complete = true;
            },
            Keysym::BackSpace => {
                self.back.pop();
            },
            Keysym::Delete => {
                self.front.pop();
            },
            Keysym::Left => {
                if let Some(c) = self.back.pop() {
                    self.front.push(c);
                }
            },
            Keysym::Right => {
                if let Some(c) = self.front.pop() {
                    self.back.push(c);
                }
            },
            _ => (),
        }
    }

    fn release(&mut self, _: Keysym)
    {
    }

    fn text(&mut self, text: &str)
    {
        for c in text.chars() {
            self.back.push(c);
        }
    }

    fn change_layout(&mut self, _: &Layout)
    {
    }
}
