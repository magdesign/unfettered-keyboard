// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

use crate::core::Configuration;
use crate::core::Display;
use crate::core::Graphics;
use crate::core::Layout;
use crate::core::Part;
use crate::core::layout::Key;
use crate::core::layout::MOD_ALT;
use crate::core::layout::MOD_CTRL;
use crate::core::layout::MOD_META;
use crate::core::layout::MODIFIERS_MAX;
use std::collections::VecDeque;
use std::convert::TryInto;
use std::iter;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;
use xkeysym::Keysym;

/*
 * The available states for the modifier keys.
 *
 * The state goes Released -> Pressed when the modifier key is pressed.
 *
 * When pressed, the modifier can be released, going Pressed -> Latched so that
 * the modifier will apply to the next key press.
 * It could be held down for long enough that it goes Pressed -> Locked, and the
 * modifier would apply to any other key press until it is released, pressed and
 * released again.
 * Finally, it could be held down while another key is being pressed, going
 * Pressed -> HeldPressed (or Locked -> HeldLockedPressed) to apply until it is
 * released.
 *
 * When latched, it could go Latched -> Released when any key
 * (including itself) is released.
 *
 * When held, it goes Held -> Released (or HeldLocked -> Released) when the
 * modifier key is released.
 *
 * When locked, the modifier key goes Locked -> HeldLocked when it is pressed
 * again.
 */
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModState {
    Released,
    Pressed,
    HeldPressed,
    Latched,
    Held,
    Locked,
    HeldLocked,
    HeldLockedPressed,
}

#[derive(PartialEq)]
enum DrawOperation {
    Key,
    Modifiers([bool; MODIFIERS_MAX]),
    Labels,
}

impl DrawOperation {
    fn from_modifier(modifier: usize) -> DrawOperation
    {
        let mut modifiers = [false; MODIFIERS_MAX];
        modifiers[modifier - 1] = true;
        DrawOperation::Modifiers(modifiers)
    }

    fn from_modifier_edge(modifier: usize) -> DrawOperation
    {
        if Layout::is_label_modifier(modifier) {
            DrawOperation::Labels
        } else {
            DrawOperation::from_modifier(modifier)
        }
    }

    fn combine(&mut self, other: Self)
    {
        match (&*self, &other) {
            (&DrawOperation::Labels, _) | (_, &DrawOperation::Key) => (),
            (_, &DrawOperation::Labels) | (&DrawOperation::Key, _) => {
                *self = other;
            },
            (&DrawOperation::Modifiers(a), &DrawOperation::Modifiers(b)) => {
                let mods = iter::zip(&a, &b).map(|(a, b)| *a || *b);
                *self = DrawOperation::Modifiers(mods.collect::<Vec<_>>().try_into().unwrap());
            },
        }
    }
}

struct Press {
    x1: f64,
    y1: f64,
    part: usize,
    timer: Instant,
}

const PRESSES_MAX: usize = 64;

pub enum FeedbackType {
    GestureStart,
    GestureEnd,
    KeyRepeat,
}

pub trait Haptics: Send {
    fn vibrate(&self, feedback_type: FeedbackType);
}

pub trait Keyboard {
    fn key_supported(&self, sym: Keysym) -> bool;

    fn press(&mut self, sym: Keysym);
    fn release(&mut self, sym: Keysym);
    fn text(&mut self, text: &str);
    fn change_layout(&mut self, layout: &Layout);
}

pub struct Button<D: Display, K: Keyboard> {
    layout: Layout,
    kbd: K,
    gfx: Arc<Mutex<Graphics<D>>>,
    haptics: Option<Box<dyn Haptics>>,

    presses: [Option<Press>; PRESSES_MAX],
    timers: VecDeque<usize>,
    modifiers: [ModState; MODIFIERS_MAX],

    longpress: Duration,
    repeat: Duration,
    emit_haptics: bool,
    r_square: f64,

    x_scale: f64,
    y_scale: f64,
}

const PART_TL: usize = 1;
const PART_TR: usize = 2;
const PART_BL: usize = 3;
const PART_BR: usize = 4;
const PART_LEFT: usize = 5;
const PART_RIGHT: usize = 6;
const PART_TOP: usize = 7;
const PART_BOTTOM: usize = 8;

const ANGLE_PARTS: [[usize; 4]; 16] = [
    [PART_LEFT, PART_TL, PART_BL, PART_TOP],
    [PART_TL, PART_LEFT, PART_TOP, PART_BL],
    [PART_TL, PART_TOP, PART_LEFT, PART_TR],
    [PART_TOP, PART_TL, PART_TR, PART_LEFT],
    [PART_TOP, PART_TR, PART_TL, PART_RIGHT],
    [PART_TR, PART_TOP, PART_RIGHT, PART_TL],
    [PART_TR, PART_RIGHT, PART_TOP, PART_BR],
    [PART_RIGHT, PART_TR, PART_BR, PART_TOP],
    [PART_RIGHT, PART_BR, PART_TR, PART_BOTTOM],
    [PART_BR, PART_RIGHT, PART_BOTTOM, PART_TR],
    [PART_BR, PART_BOTTOM, PART_RIGHT, PART_BL],
    [PART_BOTTOM, PART_BR, PART_BL, PART_RIGHT],
    [PART_BOTTOM, PART_BL, PART_BR, PART_LEFT],
    [PART_BL, PART_BOTTOM, PART_LEFT, PART_BR],
    [PART_BL, PART_LEFT, PART_BOTTOM, PART_TL],
    [PART_LEFT, PART_BL, PART_TL, PART_BOTTOM],
];

// The None option cannot be copied, but it works if it is a constant. Rust is insane.
const NO_PRESS: Option<Press> = None;

impl<D: Display, K: Keyboard> Button<D, K> {
    pub fn new(cfg: &Configuration,
               mut layout: Layout, mut kbd: K,
               gfx: Arc<Mutex<Graphics<D>>>) -> Button<D, K>
    {
        layout.update_keys_supported(&kbd);
        kbd.change_layout(&layout);

        Button {
            layout,
            kbd,
            gfx,
            haptics: None,

            presses: [NO_PRESS; PRESSES_MAX],
            modifiers: [ModState::Released; MODIFIERS_MAX],
            timers: VecDeque::with_capacity(PRESSES_MAX),

            longpress: Duration::from_millis(cfg.longpress_ms()),
            repeat: Duration::from_millis(cfg.repeat_ms()),
            emit_haptics: cfg.haptics(),
            r_square: 0.25,

            x_scale: 0.0,
            y_scale: 0.0,
        }
    }

    #[inline(always)]
    pub fn layout(&self) -> &Layout
    {
        &self.layout
    }

    #[inline(always)]
    pub fn keyboard(&self) -> &K
    {
        &self.kbd
    }

    #[inline(always)]
    pub fn keyboard_mut(&mut self) -> &mut K
    {
        &mut self.kbd
    }

    #[inline(always)]
    pub fn mod_state(&self) -> &[ModState; MODIFIERS_MAX]
    {
        &self.modifiers
    }

    #[inline(always)]
    pub fn set_text_supported(&mut self, text_supp: bool)
    {
        self.layout.set_text_supported(text_supp)
    }

    #[inline(always)]
    pub fn update_keys_supported(&mut self)
    {
        self.layout.update_keys_supported(&self.kbd)
    }

    pub fn set_haptics(&mut self, haptics: Box<dyn Haptics>)
    {
        if self.emit_haptics {
            self.haptics = Some(haptics);
        }
    }

    pub fn set_scale(&mut self, x_scale: f64, y_scale: f64)
    {
        self.x_scale = x_scale;
        self.y_scale = y_scale;
    }

    pub fn next_time(&self) -> Option<Instant>
    {
        let id = *self.timers.front()?;
        if id == PRESSES_MAX {
            return None;
        }

        // The next timer must always be held by an active key press.
        Some(self.presses[id].as_ref().unwrap().timer)
    }

    fn update_modifier(&mut self, modifier: usize, old: ModState, new: ModState) -> bool
    {
        if self.modifiers[modifier - 1] == old {
            self.modifiers[modifier - 1] = new;

            if new == ModState::Released && Layout::is_pressable_modifier(modifier) {
                self.kbd.release(Layout::modifier_keysym(modifier));
            }

            if old == ModState::Released && Layout::is_pressable_modifier(modifier) {
                self.kbd.press(Layout::modifier_keysym(modifier));
            }

            if old == ModState::Released || new == ModState::Released {
                self.layout.update_modifiers(&self.kbd, &self.modifiers);
                if Layout::is_keysym_modifier(modifier) {
                    self.kbd.change_layout(&self.layout);
                }
            }

            true
        } else {
            false
        }
    }

    fn try_press_mod(&mut self, modifier: usize) -> DrawOperation
    {
        let mut draw = DrawOperation::Key;

        if modifier != 0 {
            let changed = self.update_modifier(modifier,
                                               ModState::Released,
                                               ModState::Pressed);
            if changed && Layout::is_label_modifier(modifier) {
                draw = DrawOperation::Labels;
            }

            self.update_modifier(modifier,
                                 ModState::Latched,
                                 ModState::Held);
            self.update_modifier(modifier,
                                 ModState::Locked,
                                 ModState::HeldLocked);
        } else {
            for modifier in 1..=MODIFIERS_MAX {
                self.update_modifier(modifier,
                                     ModState::Pressed,
                                     ModState::HeldPressed);
                self.update_modifier(modifier,
                                     ModState::Locked,
                                     ModState::HeldLockedPressed);
            }
        }

        draw
    }

    fn draw(op: DrawOperation,
            gfx: &Arc<Mutex<Graphics<D>>>,
            layout: &Layout,
            key: Option<&Key>,
            mod_state: &[ModState])
    {
        let mut gfx = gfx.lock().unwrap();

        match op {
            DrawOperation::Key => {
                if let Some(key) = key {
                    gfx.draw_single(key, mod_state);
                }
            },
            DrawOperation::Modifiers(m) => {
                gfx.draw_modifiers(layout, mod_state, &m);

                if let Some(key) = key {
                    gfx.draw_single(key, mod_state);
                }
            },
            DrawOperation::Labels => {
                gfx.change_layout(layout, mod_state);
            }
        }
    }

    pub fn press(&mut self, id: usize, x: f64, y: f64)
    {
        if self.presses[id].is_some() {
            return;
        }

        let (x, y) = (x * self.x_scale, y * self.y_scale);

        let key = match self.layout.locate_key_mut(x, y) {
            Some(k) => k,
            // Ignore the press if it is not for a key.
            None => return,
        };

        key.parts[0].press();

        if let Some(h) = &self.haptics {
            h.vibrate(FeedbackType::GestureStart);
        }

        let modifier = key.parts[0].modifier_id();
        let draw = self.try_press_mod(modifier);

        let timer = Instant::now() + self.longpress;
        self.presses[id] = Some(Press {
            x1: x,
            y1: y,
            part: 0,
            timer,
        });
        self.timers.push_back(id);

        let key = self.layout.locate_key(x, y).unwrap();
        Self::draw(draw, &self.gfx, &self.layout, Some(key), &self.modifiers);
    }

    fn update_pressed_part(press: &mut Press, key: &Key, angle: usize)
    {
        if key.parts[ANGLE_PARTS[angle][0]].pressable() {
            press.part = ANGLE_PARTS[angle][0];
        } else if key.parts[ANGLE_PARTS[angle][1]].pressable() {
            press.part = ANGLE_PARTS[angle][1];
        } else if key.parts[ANGLE_PARTS[angle][2]].pressable() {
            press.part = ANGLE_PARTS[angle][2];
        } else if key.parts[ANGLE_PARTS[angle][3]].pressable() {
            press.part = ANGLE_PARTS[angle][3];
        }
    }

    fn remove_timer(&mut self, id: usize)
    {
        for evt in self.timers.iter_mut() {
            if *evt == id {
                *evt = PRESSES_MAX;
                break;
            }
        }

        while self.timers.front() == Some(&PRESSES_MAX) {
            self.timers.pop_front();
        }
    }

    fn try_cancel_mod(&mut self, modifier: usize) -> DrawOperation
    {
        let mut draw = DrawOperation::Key;

        if modifier != 0 {
            let changed = self.update_modifier(modifier,
                                               ModState::Pressed,
                                               ModState::Released);
            if changed && Layout::is_label_modifier(modifier) {
                draw = DrawOperation::Labels;
            }

            let changed = self.update_modifier(modifier,
                                               ModState::Locked,
                                               ModState::Released);
            if changed && Layout::is_label_modifier(modifier) {
                draw = DrawOperation::Labels;
            }

            let changed = self.update_modifier(modifier,
                                               ModState::HeldPressed,
                                               ModState::Released);
            if changed && Layout::is_label_modifier(modifier) {
                draw = DrawOperation::Labels;
            }

            let changed = self.update_modifier(modifier,
                                               ModState::HeldLockedPressed,
                                               ModState::Released);
            if changed && Layout::is_label_modifier(modifier) {
                draw = DrawOperation::Labels;
            }

            self.update_modifier(modifier,
                                 ModState::Held,
                                 ModState::Latched);
            self.update_modifier(modifier,
                                 ModState::HeldLocked,
                                 ModState::Locked);
        }

        draw
    }

    pub fn pos(&mut self, id: usize, x: f64, y: f64)
    {
        let press = match &mut self.presses[id] {
            Some(p) => p,
            None => return,
        };

        let (x, y) = (x * self.x_scale, y * self.y_scale);

        let (dx, dy) = (x - press.x1, y - press.y1);
        if dx * dx + dy * dy < self.r_square {
            // We only need to make changes when the key is being dragged.
            return;
        }

        let angle = dy.atan2(dx) * 8.0 / std::f64::consts::PI + 8.0;
        let angle = angle.clamp(0.0, 15.0) as usize;

        let key = self.layout.locate_key_mut(press.x1, press.y1).unwrap();

        let old_part = press.part;
        Self::update_pressed_part(press, key, angle);
        let new_part = press.part;

        if new_part == old_part {
            return;
        }

        key.parts[old_part].release();
        key.parts[new_part].press();

        if let Some(h) = &self.haptics {
            h.vibrate(FeedbackType::GestureEnd);
        }

        let modifier1 = key.parts[old_part].modifier_id();
        let modifier2 = key.parts[new_part].modifier_id();

        press.timer = Instant::now() + self.longpress;
        self.remove_timer(id);
        self.timers.push_back(id);

        let mut draw = self.try_cancel_mod(modifier1);
        draw.combine(self.try_press_mod(modifier2));

        let press = self.presses[id].as_ref().unwrap();
        let key = self.layout.locate_key(press.x1, press.y1).unwrap();
        Self::draw(draw, &self.gfx, &self.layout, Some(key), &self.modifiers);
    }

    fn emit(kbd: &mut K, part: &Part, modifiers: &[ModState])
    {
        let ctrl_pressed = modifiers[MOD_CTRL - 1] != ModState::Released;
        let alt_pressed = modifiers[MOD_ALT - 1] != ModState::Released;
        let meta_pressed = modifiers[MOD_META - 1] != ModState::Released;
        let mods_pressed = ctrl_pressed || alt_pressed || meta_pressed;

        if part.sym() == Keysym::Scroll_Lock
        || part.sym() == Keysym::Num_Lock
        || part.sym() == Keysym::Caps_Lock {
            return;
        }

        if part.key_available() && mods_pressed {
            kbd.press(part.sym());
            kbd.release(part.sym());
        } else if part.text_available() {
            kbd.text(part.text());
        } else if part.key_available() {
            kbd.press(part.sym());
            kbd.release(part.sym());
        }
    }

    fn release_layout_switcher(&mut self, sym: Keysym) -> DrawOperation
    {
        let old_width = self.layout.width();
        let old_height = self.layout.height();

        match sym {
            Keysym::Scroll_Lock => {
                self.layout.switch_greekmath();
            },
            Keysym::Num_Lock => {
                self.layout.switch_numeric();
            },
            Keysym::Caps_Lock => {
                self.layout.switch_text();
            },
            Keysym::Menu => {
                self.layout.switch_emoji();
            },
            _ => return DrawOperation::Key,
        }

        self.layout.update_keys_supported(&self.kbd);
        self.kbd.change_layout(&self.layout);

        self.x_scale *= self.layout.width() / old_width;
        self.y_scale *= self.layout.height() / old_height;

        DrawOperation::Labels
    }

    fn is_normal_key_pressed(&self) -> bool
    {
        for id in &self.timers {
            if *id == PRESSES_MAX {
                continue;
            }

            let press = self.presses[*id].as_ref().unwrap();
            let key = self.layout.locate_key(press.x1, press.y1).unwrap();

            if key.parts[press.part].modifier_id() == 0 {
                return true;
            }
        }

        false
    }

    /*
     * Release the given modifier key. If the key being pressed is not a
     * modifier key (i.e. modifier = 0), release all latched modifier keys.
     */
    fn release_mod(&mut self, modifier: usize) -> DrawOperation
    {
        let mut draw = DrawOperation::Key;

        if modifier != 0 {
            self.update_modifier(modifier,
                                 ModState::Pressed,
                                 ModState::Latched);

            self.update_modifier(modifier,
                                 ModState::HeldPressed,
                                 ModState::Held);

            self.update_modifier(modifier,
                                 ModState::HeldLockedPressed,
                                 ModState::HeldLocked);

            if self.is_normal_key_pressed() {
                self.update_modifier(modifier,
                                     ModState::Held,
                                     ModState::Latched);

                self.update_modifier(modifier,
                                     ModState::HeldLocked,
                                     ModState::Locked);
            }

            let changed = self.update_modifier(modifier,
                                               ModState::Held,
                                               ModState::Released);
            if changed && Layout::is_label_modifier(modifier) {
                draw = DrawOperation::Labels;
            }

            let changed = self.update_modifier(modifier,
                                               ModState::HeldLocked,
                                               ModState::Released);
            if changed && Layout::is_label_modifier(modifier) {
                draw = DrawOperation::Labels;
            }
        } else {
            for modifier in 1..=MODIFIERS_MAX {
                let changed = self.update_modifier(modifier,
                                                   ModState::Latched,
                                                   ModState::Released);
                if changed {
                    draw.combine(DrawOperation::from_modifier_edge(modifier));
                }
            }
        }

        draw
    }

    pub fn release(&mut self, id: usize)
    {
        let press = match &mut self.presses[id] {
            Some(p) => p,
            None => return,
        };

        let key = self.layout.locate_key_mut(press.x1, press.y1).unwrap();

        key.parts[press.part].release();

        let keysym = key.parts[press.part].sym();
        let modifier = key.parts[press.part].modifier_id();
        if modifier == 0 {
            Self::emit(&mut self.kbd, &key.parts[press.part], &self.modifiers);
        }

        let mut draw = self.release_mod(modifier);
        draw.combine(self.release_layout_switcher(keysym));

        let press = self.presses[id].as_ref().unwrap();
        let key = self.layout.locate_key(press.x1, press.y1).unwrap();
        Self::draw(draw, &self.gfx, &self.layout, Some(key), &self.modifiers);

        self.remove_timer(id);
        self.presses[id] = None;
    }

    fn lock_mod(&mut self, modifier: usize) -> DrawOperation
    {
        let changed = self.update_modifier(modifier, ModState::Pressed, ModState::Locked);
        if changed {
            DrawOperation::from_modifier(modifier)
        } else {
            DrawOperation::Key
        }
    }

    pub fn dispatch_timers(&mut self)
    {
        let mut draw = DrawOperation::Key;

        while !self.timers.is_empty() {
            let id = *self.timers.front().unwrap();
            if id == PRESSES_MAX {
                self.timers.pop_front();
                continue;
            }

            let press = self.presses[id].as_mut().unwrap();
            if Instant::now() < press.timer {
                break;
            }

            self.timers.pop_front();

            let key = self.layout.locate_key(press.x1, press.y1).unwrap();

            let modifier = key.parts[press.part].modifier_id();
            if modifier != 0 {
                draw.combine(self.lock_mod(modifier));
            } else {
                Self::emit(&mut self.kbd, &key.parts[press.part], &self.modifiers);

                if let Some(h) = &self.haptics {
                    h.vibrate(FeedbackType::KeyRepeat);
                }

                press.timer += self.repeat;
                self.timers.push_back(id);
            }
        }

        Self::draw(draw, &self.gfx, &self.layout, None, &self.modifiers);
    }
}
