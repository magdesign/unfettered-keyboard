// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

use crate::core::Button;
use crate::core::Configuration;
use crate::core::Display;
use crate::core::Graphics;
use crate::core::Keyboard;
use crate::core::Layout;
use crate::core::ModState;
use crate::core::PRESSES_MAX;
use crate::wayland::Dispatcher;
use std::sync::Arc;
use std::sync::Mutex;
use wayland_client::protocol::wl_pointer;
use wayland_client::protocol::wl_seat;
use wayland_client::protocol::wl_touch::WlTouch;
use wayland_client::QueueHandle;

enum PressAction {
    Pos(f64, f64),
    Press(f64, f64),
    Release,
}

struct TouchAction {
    id: usize,
    act: PressAction,
}

pub struct Seat<D: Display, K: Keyboard> {
    seat: wl_seat::WlSeat,
    queue: QueueHandle<Dispatcher>,

    ptr: Option<wl_pointer::WlPointer>,
    touch: Option<WlTouch>,

    button: Button<D, K>,

    mouse_x: f64,
    mouse_y: f64,
    actions: Vec<TouchAction>,

    /*
     * Compositor touch IDs are arbitrary 32-bit values: some compositors send
     * -1, others ever-increasing numbers. Each active touch point is given a
     * press slot from 1 to PRESSES_MAX - 1 (slot 0 is the pointer), and
     * touch_ids[slot] holds the compositor's ID for it.
     */
    touch_ids: [Option<i32>; PRESSES_MAX],
}

impl<D: Display, K: Keyboard> Seat<D, K> {
    pub fn new(cfg: &Configuration,
               layout: Layout, kbd: K, gfx: Arc<Mutex<Graphics<D>>>,
               queue: QueueHandle<Dispatcher>, seat: wl_seat::WlSeat) -> Self
    {
        let actions = Vec::new();
        let button = Button::new(cfg, layout, kbd, gfx);

        Self {
            seat,
            queue,

            ptr: None,
            touch: None,

            button,

            mouse_x: 0.0,
            mouse_y: 0.0,
            actions,

            touch_ids: [None; PRESSES_MAX],
        }
    }

    #[inline(always)]
    pub fn button(&self) -> &Button<D, K>
    {
        &self.button
    }

    #[inline(always)]
    pub fn button_mut(&mut self) -> &mut Button<D, K>
    {
        &mut self.button
    }

    #[inline(always)]
    pub fn layout(&self) -> &Layout
    {
        self.button.layout()
    }

    #[inline(always)]
    pub fn keyboard(&self) -> &K
    {
        self.button.keyboard()
    }

    #[inline(always)]
    pub fn keyboard_mut(&mut self) -> &mut K
    {
        self.button.keyboard_mut()
    }

    #[inline(always)]
    pub fn mod_state(&self) -> &[ModState]
    {
        self.button.mod_state()
    }

    #[inline(always)]
    pub fn set_text_supported(&mut self, text_supp: bool)
    {
        self.button.set_text_supported(text_supp)
    }

    #[inline(always)]
    pub fn set_scale(&mut self, x_scale: f64, y_scale: f64)
    {
        self.button.set_scale(x_scale, y_scale);
    }

    pub fn set_capabilities(&mut self, caps: wl_seat::Capability)
    {
        if caps.contains(wl_seat::Capability::Pointer) {
            self.ptr = Some(self.seat.get_pointer(&self.queue, ()));
        }

        if caps.contains(wl_seat::Capability::Touch) {
            self.touch = Some(self.seat.get_touch(&self.queue, ()));
        }
    }

    pub fn ptr_motion(&mut self, x: f64, y: f64)
    {
        self.mouse_x = x;
        self.mouse_y = y;

        let act = TouchAction { id: 0, act: PressAction::Pos(x, y), };
        self.actions.push(act);
    }

    pub fn ptr_button(&mut self, state: wl_pointer::ButtonState)
    {
        match state {
            wl_pointer::ButtonState::Pressed => {
                let (x, y) = (self.mouse_x, self.mouse_y);
                let act = TouchAction { id: 0, act: PressAction::Press(x, y), };
                self.actions.push(act);
            },
            wl_pointer::ButtonState::Released => {
                let act = TouchAction { id: 0, act: PressAction::Release, };
                self.actions.push(act);
            },
            _ => eprintln!("warn: ignoring unknown pointer event"),
        }
    }

    // The press slot of an active touch point.
    fn touch_slot(&self, id: i32) -> Option<usize>
    {
        (1..PRESSES_MAX).find(|&slot| self.touch_ids[slot] == Some(id))
    }

    pub fn touch_press(&mut self, id: i32, x: f64, y: f64)
    {
        // A repeated down for an active ID keeps its slot.
        let slot = match self.touch_slot(id) {
            Some(slot) => slot,
            None => match (1..PRESSES_MAX).find(|&slot| self.touch_ids[slot].is_none()) {
                Some(slot) => slot,
                None => {
                    eprintln!("warn: too many touch points, ignoring touch {}", id);
                    return;
                },
            },
        };

        self.touch_ids[slot] = Some(id);
        let act = TouchAction { id: slot, act: PressAction::Press(x, y), };
        self.actions.push(act);
    }

    pub fn touch_pos(&mut self, id: i32, x: f64, y: f64)
    {
        if let Some(slot) = self.touch_slot(id) {
            let act = TouchAction { id: slot, act: PressAction::Pos(x, y), };
            self.actions.push(act);
        }
    }

    pub fn touch_release(&mut self, id: i32)
    {
        if let Some(slot) = self.touch_slot(id) {
            /*
             * The slot can be reused right away: the release is queued before
             * any later press in this frame, and actions run in order.
             */
            self.touch_ids[slot] = None;
            let act = TouchAction { id: slot, act: PressAction::Release, };
            self.actions.push(act);
        }
    }

    /*
     * The compositor took over all touch points (e.g. for a gesture). No up
     * events will follow for them, so forget them without emitting keys.
     */
    pub fn touch_cancel(&mut self)
    {
        // Pending pointer actions (slot 0) are unaffected.
        self.actions.retain(|act| act.id == 0);
        self.touch_ids = [None; PRESSES_MAX];

        for slot in 1..PRESSES_MAX {
            self.button.cancel(slot);
        }
    }

    /*
     * Forget every press, e.g. when the keyboard is hidden while a finger is
     * down and the compositor will not send the release anymore.
     */
    pub fn cancel_all(&mut self)
    {
        self.actions.clear();
        self.touch_ids = [None; PRESSES_MAX];
        self.button.cancel_all();
    }

    pub fn commit(&mut self)
    {
        for action in &self.actions {
            match action.act {
                PressAction::Press(x, y) => {
                    self.button.press(action.id, x, y);
                },
                PressAction::Pos(x, y) => {
                    self.button.pos(action.id, x, y);
                },
                PressAction::Release => {
                    self.button.release(action.id);
                },
            }
        }

        // Reset the action for the next frame.
        self.actions.clear();
    }
}
