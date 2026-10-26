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

    pub fn touch_press(&mut self, id: i32, x: f64, y: f64)
    {
        let id = id as usize + 1;
        let act = TouchAction { id, act: PressAction::Press(x, y), };
        self.actions.push(act);
    }

    pub fn touch_pos(&mut self, id: i32, x: f64, y: f64)
    {
        let id = id as usize + 1;
        let act = TouchAction { id, act: PressAction::Pos(x, y), };
        self.actions.push(act);
    }

    pub fn touch_release(&mut self, id: i32)
    {
        let id = id as usize + 1;
        let act = TouchAction { id, act: PressAction::Release, };
        self.actions.push(act);
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
