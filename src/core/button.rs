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
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
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
    // Starting position, in layout units, used to compute the drag angle.
    x1: f64,
    y1: f64,
    // The key being pressed, as an index into the current layout's rows.
    row: usize,
    col: usize,
    part: usize,
    timer: Instant,
}

pub const PRESSES_MAX: usize = 64;

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
    // IDs of presses with a pending timer, sorted by the time they expire.
    timers: VecDeque<usize>,
    modifiers: [ModState; MODIFIERS_MAX],

    longpress: Duration,
    repeat: Duration,
    emit_haptics: bool,
    // Command run by the script key, from "script" in the configuration.
    script: Option<String>,
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
            script: cfg.script().map(String::from),
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

        match &self.presses[id] {
            Some(press) => Some(press.timer),
            // Stale entry: wake up now so dispatch_timers() can discard it.
            None => Some(Instant::now()),
        }
    }

    fn timer_of(&self, id: usize) -> Option<Instant>
    {
        self.presses.get(id)?.as_ref().map(|p| p.timer)
    }

    /*
     * Queue the timer of an active press, keeping the queue sorted so that the
     * front always holds the earliest timer. The press must not be queued yet.
     */
    fn schedule(&mut self, id: usize)
    {
        let timer = match self.timer_of(id) {
            Some(t) => t,
            None => return,
        };

        let pos = self.timers.iter()
                      .position(|&other| self.timer_of(other).map_or(false, |t| t > timer))
                      .unwrap_or(self.timers.len());
        self.timers.insert(pos, id);
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
        // Ignore IDs that cannot be tracked and presses that are already active.
        match self.presses.get(id) {
            Some(None) => (),
            _ => return,
        }

        let (x, y) = (x * self.x_scale, y * self.y_scale);

        let (row, col) = match self.layout.locate_key_index(x, y) {
            Some(idx) => idx,
            // Ignore the press if it is not for a key.
            None => return,
        };

        let key = match self.layout.key_mut(row, col) {
            Some(k) => k,
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
            row,
            col,
            part: 0,
            timer,
        });
        self.schedule(id);

        let key = self.layout.key(row, col);
        Self::draw(draw, &self.gfx, &self.layout, key, &self.modifiers);
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
        self.timers.retain(|&t| t != id);
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
        let press = match self.presses.get_mut(id) {
            Some(Some(p)) => p,
            _ => return,
        };

        let (x, y) = (x * self.x_scale, y * self.y_scale);

        let (dx, dy) = (x - press.x1, y - press.y1);
        // The negated comparison also bails out on NaN.
        if !(dx * dx + dy * dy >= self.r_square) {
            // We only need to make changes when the key is being dragged.
            return;
        }

        let angle = dy.atan2(dx) * 8.0 / std::f64::consts::PI + 8.0;
        let angle = angle.clamp(0.0, 15.0) as usize;

        let key = match self.layout.key_mut(press.row, press.col) {
            Some(k) => k,
            None => return,
        };

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
        let (row, col) = (press.row, press.col);
        self.remove_timer(id);
        self.schedule(id);

        let mut draw = self.try_cancel_mod(modifier1);
        draw.combine(self.try_press_mod(modifier2));

        let key = self.layout.key(row, col);
        Self::draw(draw, &self.gfx, &self.layout, key, &self.modifiers);
    }

    fn emit(kbd: &mut K, part: &Part, modifiers: &[ModState])
    {
        let ctrl_pressed = modifiers[MOD_CTRL - 1] != ModState::Released;
        let alt_pressed = modifiers[MOD_ALT - 1] != ModState::Released;
        let meta_pressed = modifiers[MOD_META - 1] != ModState::Released;
        let mods_pressed = ctrl_pressed || alt_pressed || meta_pressed;

        // Action keys act on release in release() and emit nothing.
        if Layout::is_action(part.sym()) {
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

    /*
     * Switch to the layout selected by a layout switcher keysym. The caller
     * must have already removed its own press.
     */
    fn switch_layout(&mut self, sym: Keysym) -> DrawOperation
    {
        if !Layout::is_layout_switcher(sym) {
            return DrawOperation::Key;
        }

        /*
         * Other presses refer to keys of the old layout, which will not exist
         * anymore. Cancel them, so that they neither emit a different key nor
         * leave a modifier pressed.
         */
        for other in 0..PRESSES_MAX {
            self.cancel_press(other);
        }

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

        // An empty layout has no size; keep the old scale instead of NaN/inf.
        let x_ratio = self.layout.width() / old_width;
        let y_ratio = self.layout.height() / old_height;
        if x_ratio.is_finite() {
            self.x_scale *= x_ratio;
        }
        if y_ratio.is_finite() {
            self.y_scale *= y_ratio;
        }

        DrawOperation::Labels
    }

    fn is_normal_key_pressed(&self) -> bool
    {
        self.presses.iter().flatten().any(|press| {
            self.layout.key(press.row, press.col)
                .map_or(false, |key| key.parts[press.part].modifier_id() == 0)
        })
    }

    /*
     * Forget a press without emitting anything. A modifier that was only
     * pressed (not latched or locked by an earlier press) is released, like
     * when dragging away from it. Returns what needs to be redrawn and the
     * key that the press was on.
     */
    fn cancel_press(&mut self, id: usize) -> (DrawOperation, Option<(usize, usize)>)
    {
        let press = match self.presses.get_mut(id).and_then(Option::take) {
            Some(p) => p,
            None => return (DrawOperation::Key, None),
        };

        self.remove_timer(id);

        let modifier = match self.layout.key_mut(press.row, press.col) {
            Some(key) => {
                let part = &mut key.parts[press.part];
                part.release();
                part.modifier_id()
            },
            None => 0,
        };

        (self.try_cancel_mod(modifier), Some((press.row, press.col)))
    }

    /*
     * Cancel a press, e.g. when the compositor cancels a touch point. Unlike
     * release(), this never emits a key.
     */
    pub fn cancel(&mut self, id: usize)
    {
        let (draw, idx) = self.cancel_press(id);
        if let Some((row, col)) = idx {
            let key = self.layout.key(row, col);
            Self::draw(draw, &self.gfx, &self.layout, key, &self.modifiers);
        }
    }

    pub fn cancel_all(&mut self)
    {
        for id in 0..PRESSES_MAX {
            self.cancel(id);
        }
    }

    #[inline(always)]
    pub fn is_pressed(&self) -> bool
    {
        self.presses.iter().any(Option::is_some)
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
        let (row, col, part) = match self.presses.get(id) {
            Some(Some(p)) => (p.row, p.col, p.part),
            _ => return,
        };

        let key = match self.layout.key_mut(row, col) {
            Some(k) => k,
            None => {
                // The key is gone; drop the press without emitting anything.
                self.cancel(id);
                return;
            },
        };

        key.parts[part].release();

        let keysym = key.parts[part].sym();
        let modifier = key.parts[part].modifier_id();
        if modifier == 0 {
            Self::emit(&mut self.kbd, &key.parts[part], &self.modifiers);
        }

        let mut draw = self.release_mod(modifier);

        // The press must be gone before switching, so it is not cancelled.
        self.remove_timer(id);
        self.presses[id] = None;

        let switched = modifier == 0 && Layout::is_layout_switcher(keysym);
        if switched {
            draw.combine(self.switch_layout(keysym));
        }

        if modifier == 0 && Layout::is_script(keysym) {
            self.run_script();
        }

        // After a switch, the indices refer to a different layout.
        let key = if switched { None } else { self.layout.key(row, col) };
        Self::draw(draw, &self.gfx, &self.layout, key, &self.modifiers);
    }

    /*
     * Start the configured script with sh -c, without waiting for it: the
     * keyboard must stay responsive while it runs. A thread reaps it.
     */
    fn run_script(&self)
    {
        let script = match &self.script {
            Some(s) => s,
            None => return,
        };

        let child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .spawn();

        match child {
            Ok(mut child) => {
                thread::spawn(move || {
                    let _ = child.wait();
                });
            },
            Err(e) => eprintln!("warn: failed to run script {:?}: {}", script, e),
        }
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

        /*
         * Compare against a single point in time, and always reschedule
         * repeats after it, so that this loop handles each timer at most once
         * per call even if emitting takes longer than the repeat interval.
         */
        let now = Instant::now();
        let repeat = self.repeat.max(Duration::from_millis(1));

        while let Some(&id) = self.timers.front() {
            let press = match self.presses.get_mut(id) {
                Some(Some(p)) => p,
                _ => {
                    // Stale entry for a press that no longer exists.
                    self.timers.pop_front();
                    continue;
                },
            };

            if now < press.timer {
                break;
            }

            self.timers.pop_front();

            let key = match self.layout.key(press.row, press.col) {
                Some(k) => k,
                None => continue,
            };
            let part = &key.parts[press.part];

            let modifier = part.modifier_id();
            if modifier != 0 {
                draw.combine(self.lock_mod(modifier));
            } else if Layout::is_action(part.sym()) {
                // Action keys act on release; holding them must not repeat.
            } else {
                Self::emit(&mut self.kbd, part, &self.modifiers);

                if let Some(h) = &self.haptics {
                    h.vibrate(FeedbackType::KeyRepeat);
                }

                // Skip missed repeats instead of emitting them in a burst.
                press.timer += repeat;
                if press.timer <= now {
                    press.timer = now + repeat;
                }
                self.schedule(id);
            }
        }

        Self::draw(draw, &self.gfx, &self.layout, None, &self.modifiers);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imgref::ImgRefMut;
    use rgb::alt::BGRA;
    use std::path::Path;
    use std::thread;

    #[derive(Debug, PartialEq)]
    enum Ev {
        Press(Keysym),
        Release(Keysym),
        Text(String),
    }

    #[derive(Default)]
    struct TestKeyboard {
        events: Vec<Ev>,
    }

    impl Keyboard for TestKeyboard {
        fn key_supported(&self, sym: Keysym) -> bool
        {
            sym.name().map_or(false, |n| n.starts_with("XK_"))
        }

        fn press(&mut self, sym: Keysym)
        {
            self.events.push(Ev::Press(sym));
        }

        fn release(&mut self, sym: Keysym)
        {
            self.events.push(Ev::Release(sym));
        }

        fn text(&mut self, text: &str)
        {
            self.events.push(Ev::Text(text.to_string()));
        }

        fn change_layout(&mut self, _layout: &Layout)
        {
        }
    }

    struct TestDisplay {
        width: u32,
        height: u32,
        buf: Vec<BGRA<u8>>,
    }

    impl Display for TestDisplay {
        fn size(&self) -> (u32, u32)
        {
            (self.width, self.height)
        }

        fn begin<'a>(&'a mut self) -> ImgRefMut<'a, BGRA<u8>>
        {
            ImgRefMut::new(&mut self.buf, self.width as usize, self.height as usize)
        }

        fn resize(&mut self, width: u32, height: u32)
        {
            self.width = width;
            self.height = height;
            self.buf.resize(width as usize * height as usize, BGRA { b: 0, g: 0, r: 0, a: 0 });
        }

        fn end(&mut self, _x: u32, _y: u32, _width: u32, _height: u32)
        {
        }
    }

    type TestButton = Button<TestDisplay, TestKeyboard>;

    // A button using the shipped layouts, taking coordinates in layout units.
    fn setup(yaml: &str) -> TestButton
    {
        let cfg = Configuration::from_yaml_str(yaml).unwrap();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/layouts");
        let layout = Layout::load_from_dir(&cfg, &dir).unwrap();

        let disp = TestDisplay { width: 1, height: 1, buf: vec![BGRA { b: 0, g: 0, r: 0, a: 0 }] };
        let gfx = Arc::new(Mutex::new(Graphics::new(disp, &cfg)));

        let mut btn = Button::new(&cfg, layout, TestKeyboard::default(), gfx.clone());
        btn.set_text_supported(true);
        gfx.lock().unwrap().resize(btn.layout(), btn.mod_state(), 400, 200);
        btn.set_scale(1.0, 1.0);
        btn
    }

    // Center of the key with a part for the keysym, and that part's index.
    fn find(btn: &TestButton, sym: Keysym) -> (f64, f64, usize)
    {
        for row in btn.layout().rows() {
            for key in row {
                if let Some(i) = key.parts.iter().position(|p| p.sym() == sym) {
                    return ((key.x1 + key.x2) / 2.0, (key.y1 + key.y2) / 2.0, i);
                }
            }
        }

        panic!("{:?} is not in the layout", sym);
    }

    // Direction to drag for each part, see the constants above.
    fn drag_offset(part: usize) -> (f64, f64)
    {
        match part {
            0 => (0.0, 0.0),
            PART_TL => (-0.6, -0.6),
            PART_TR => (0.6, -0.6),
            PART_BL => (-0.6, 0.6),
            PART_BR => (0.6, 0.6),
            PART_LEFT => (-0.6, 0.0),
            PART_RIGHT => (0.6, 0.0),
            PART_TOP => (0.0, -0.6),
            PART_BOTTOM => (0.0, 0.6),
            _ => unreachable!(),
        }
    }

    // Press and drag to the part with the keysym, without releasing.
    fn hold(btn: &mut TestButton, id: usize, sym: Keysym)
    {
        let (x, y, part) = find(btn, sym);
        btn.press(id, x, y);

        if part != 0 {
            let (dx, dy) = drag_offset(part);
            btn.pos(id, x + dx, y + dy);
        }
    }

    fn tap(btn: &mut TestButton, id: usize, sym: Keysym)
    {
        hold(btn, id, sym);
        btn.release(id);

        // A layout switch rescales input to the new layout; keep using layout units.
        btn.set_scale(1.0, 1.0);
    }

    /*
     * Events other than modifier presses. Some switchers are swiped from a
     * modifier key (e.g. 123+ from Ctrl), which presses and releases it.
     */
    fn typed(btn: &TestButton) -> Vec<&Ev>
    {
        btn.keyboard().events.iter().filter(|ev| match ev {
            Ev::Press(sym) | Ev::Release(sym) => {
                !matches!(*sym, Keysym::Shift_L | Keysym::Control_L | Keysym::Alt_L)
            },
            Ev::Text(_) => true,
        }).collect()
    }

    fn has_sym(btn: &TestButton, sym: Keysym) -> bool
    {
        btn.layout().rows().iter().flatten()
            .any(|k| k.parts.iter().any(|p| p.sym() == sym))
    }

    #[test]
    fn typing_a_key_emits_text()
    {
        let mut btn = setup("");
        tap(&mut btn, 1, Keysym::q);
        assert_eq!(btn.keyboard().events, vec![Ev::Text(String::from("q"))]);
    }

    // The emoji switcher used to type its own label ("🙂") or a Menu key.
    #[test]
    fn layout_switchers_emit_nothing()
    {
        let mut btn = setup("");
        assert!(!has_sym(&btn, Keysym::Caps_Lock));

        tap(&mut btn, 1, Keysym::Menu);
        assert!(has_sym(&btn, Keysym::Caps_Lock), "emoji layout was not opened");

        tap(&mut btn, 1, Keysym::Caps_Lock);
        assert!(!has_sym(&btn, Keysym::Caps_Lock), "text layout was not opened");

        tap(&mut btn, 1, Keysym::Num_Lock);
        assert!(has_sym(&btn, Keysym::Scroll_Lock), "numeric layout was not opened");

        assert_eq!(typed(&btn), Vec::<&Ev>::new());
    }

    #[test]
    fn holding_a_layout_switcher_does_not_repeat()
    {
        let mut btn = setup("longpress_ms: 0\nrepeat_ms: 1");
        hold(&mut btn, 1, Keysym::Num_Lock);

        btn.dispatch_timers();
        thread::sleep(Duration::from_millis(5));
        btn.dispatch_timers();

        assert_eq!(typed(&btn), Vec::<&Ev>::new());
        assert_eq!(btn.next_time(), None);
    }

    // A press on the old layout used to panic or emit another key on release.
    #[test]
    fn layout_switch_cancels_other_presses()
    {
        let mut btn = setup("longpress_ms: 100000");

        let (qx, qy, _) = find(&btn, Keysym::q);
        let (sx, sy, _) = find(&btn, Keysym::space);
        btn.press(1, qx, qy);
        btn.press(2, sx, sy);

        tap(&mut btn, 3, Keysym::Num_Lock);
        assert!(has_sym(&btn, Keysym::Scroll_Lock), "numeric layout was not opened");

        btn.pos(1, qx + 0.7, qy);
        btn.release(1);
        btn.release(2);
        btn.dispatch_timers();

        assert_eq!(typed(&btn), Vec::<&Ev>::new());
        assert!(!btn.is_pressed());

        // The new layout still works.
        tap(&mut btn, 1, Keysym::_1);
        assert_eq!(typed(&btn), vec![&Ev::Text(String::from("1"))]);
    }

    #[test]
    fn layout_switch_releases_held_modifier()
    {
        let mut btn = setup("longpress_ms: 100000");

        hold(&mut btn, 1, Keysym::Shift_L);
        assert_eq!(btn.mod_state()[0], ModState::Pressed);

        tap(&mut btn, 2, Keysym::Num_Lock);

        assert!(btn.mod_state().iter().all(|&m| m == ModState::Released));
        assert_eq!(btn.keyboard().events.last(), Some(&Ev::Release(Keysym::Shift_L)));
        btn.release(1);
    }

    // Cancelled touches (e.g. taken over by a compositor gesture) must not
    // emit anything or keep repeating.
    #[test]
    fn cancelled_press_emits_nothing()
    {
        let mut btn = setup("longpress_ms: 0\nrepeat_ms: 1");

        hold(&mut btn, 1, Keysym::BackSpace);
        btn.cancel(1);
        assert_eq!(btn.next_time(), None);

        btn.dispatch_timers();
        btn.release(1);

        assert_eq!(btn.keyboard().events, vec![]);
        assert!(!btn.is_pressed());
    }

    #[test]
    fn cancelled_modifier_is_released()
    {
        let mut btn = setup("");

        hold(&mut btn, 1, Keysym::Shift_L);
        btn.cancel_all();

        assert!(btn.mod_state().iter().all(|&m| m == ModState::Released));
        assert_eq!(btn.keyboard().events,
                   vec![Ev::Press(Keysym::Shift_L), Ev::Release(Keysym::Shift_L)]);
    }

    // Missed repeats used to be emitted in a burst after a stall.
    #[test]
    fn repeat_does_not_burst_after_a_stall()
    {
        let mut btn = setup("longpress_ms: 0\nrepeat_ms: 1");
        let (x, y, _) = find(&btn, Keysym::q);

        btn.press(1, x, y);
        btn.dispatch_timers();
        assert_eq!(btn.keyboard().events.len(), 1);

        thread::sleep(Duration::from_millis(30));
        btn.dispatch_timers();
        assert_eq!(btn.keyboard().events.len(), 2);

        btn.release(1);
        assert_eq!(btn.keyboard().events.len(), 3);
    }

    // With several keys held, a later timer must not delay an earlier one.
    #[test]
    fn timers_stay_sorted()
    {
        let mut btn = setup("longpress_ms: 50\nrepeat_ms: 1");
        let (qx, qy, _) = find(&btn, Keysym::q);
        let (wx, wy, _) = find(&btn, Keysym::w);

        btn.press(1, qx, qy);
        thread::sleep(Duration::from_millis(60));
        btn.dispatch_timers();

        // q now repeats every 1 ms, w is pressed with a 50 ms long-press.
        btn.press(2, wx, wy);
        let next = btn.next_time().unwrap();
        assert!(next <= Instant::now() + Duration::from_millis(5),
                "q's repeat is queued behind w's long-press");

        btn.release(1);
        btn.release(2);
    }

    #[test]
    fn invalid_ids_and_coordinates_are_ignored()
    {
        let mut btn = setup("");

        for id in [PRESSES_MAX, PRESSES_MAX + 1, usize::MAX] {
            btn.press(id, 0.5, 0.5);
            btn.pos(id, 1.5, 0.5);
            btn.release(id);
            btn.cancel(id);
        }

        btn.press(1, f64::NAN, f64::NAN);
        btn.press(2, f64::INFINITY, 0.5);
        btn.press(3, 0.5, -1.0);
        btn.press(4, 0.5, 1000.0);

        assert!(!btn.is_pressed());
        assert_eq!(btn.keyboard().events, vec![]);
    }

    #[test]
    fn script_key_runs_the_configured_command()
    {
        let marker = std::env::temp_dir()
            .join(format!("ufkbd-script-test-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);

        let yaml = format!("longpress_ms: 0\nrepeat_ms: 1\nscript: \"touch '{}'\"",
                           marker.display());
        let mut btn = setup(&yaml);
        assert!(has_sym(&btn, Keysym::XF86_Launch0), "script key is not on the layout");

        // Holding it neither repeats nor runs the script before release.
        hold(&mut btn, 1, Keysym::XF86_Launch0);
        btn.dispatch_timers();
        thread::sleep(Duration::from_millis(5));
        btn.dispatch_timers();
        assert_eq!(btn.next_time(), None);

        btn.release(1);

        // The script runs in the background.
        let mut ran = false;
        for _ in 0..200 {
            if marker.exists() {
                ran = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = std::fs::remove_file(&marker);

        assert!(ran, "script did not run");
        assert_eq!(typed(&btn), Vec::<&Ev>::new());
    }

    #[test]
    fn script_key_is_hidden_without_a_script()
    {
        let btn = setup("");
        assert!(!has_sym(&btn, Keysym::XF86_Launch0));

        let btn = setup("script: \"\"");
        assert!(!has_sym(&btn, Keysym::XF86_Launch0));
    }
}

