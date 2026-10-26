// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

use crate::core::Button;
use crate::core::Configuration;
use crate::core::Display;
use crate::core::Keyboard;
use evdev::AbsoluteAxisCode;
use evdev::Device;
use evdev::EventSummary;
use std::fs;
use std::io::Error;
use std::os::fd::AsFd;
use std::os::fd::AsRawFd;
use std::os::fd::BorrowedFd;
use std::os::fd::RawFd;

#[derive(Debug)]
enum PressAction {
    Pos,
    Press,
    Release,
}

#[derive(Debug)]
struct TouchAction {
    id: usize,
    act: PressAction,
}

pub struct Touchscreen<D: Display, K: Keyboard> {
    dev: Device,
    btn: Button<D, K>,
    actions: Vec<TouchAction>,
    pos: Vec<(f64, f64)>,
    slot: usize,
    y_off: f64,
}

impl<D: Display, K: Keyboard> Touchscreen<D, K> {
    fn open_device_if_supported(dent: Result<fs::DirEntry, Error>) -> Option<Device>
    {
        let path = dent.ok()?.path();
        let dev = Device::open(path).ok()?;

        let abs_props = dev.supported_absolute_axes()?;

        if abs_props.contains(AbsoluteAxisCode::ABS_MT_SLOT)
        && abs_props.contains(AbsoluteAxisCode::ABS_MT_TRACKING_ID)
        && abs_props.contains(AbsoluteAxisCode::ABS_MT_POSITION_X)
        && abs_props.contains(AbsoluteAxisCode::ABS_MT_POSITION_Y) {
            Some(dev)
        } else {
            None
        }
    }

    pub fn new(mut btn: Button<D, K>, cfg: &Configuration) -> Result<Self, Error>
    {
        let mut devs = fs::read_dir("/dev/input")?;
        let dev = devs.find_map(Self::open_device_if_supported).unwrap();

        dev.set_nonblocking(true)?;

        let props = dev.get_abs_state()?;
        let max_slots = props[AbsoluteAxisCode::ABS_MT_SLOT.0 as usize].maximum as usize + 1;

        // Dimensions of the touchscreen device
        let d_width = props[AbsoluteAxisCode::ABS_MT_POSITION_X.0 as usize].maximum;
        let d_width = d_width as f64 + 1.0;
        let d_height = props[AbsoluteAxisCode::ABS_MT_POSITION_Y.0 as usize].maximum;
        let d_height = d_height as f64 + 1.0;

        // Dimensions of the keyboard
        let c_height = d_height * cfg.evfb_height();

        // Dimensions of the keyboard layout
        let l_width = btn.layout().width();
        let l_height = btn.layout().height();

        btn.set_scale(l_width / d_width, l_height / c_height);

        Ok(Self {
            dev, btn,
            actions: Vec::new(),
            pos: vec![(0.0, 0.0); max_slots],
            slot: 0,
            y_off: d_height - c_height,
        })
    }

    #[inline(always)]
    pub fn button(&self) -> &Button<D, K>
    {
        &self.btn
    }

    pub fn dispatch_timers(&mut self)
    {
        self.btn.dispatch_timers()
    }

    fn commit(&mut self)
    {
        for act in &self.actions {
            match act.act {
                PressAction::Pos => {
                    self.btn.pos(act.id,
                                 self.pos[act.id].0,
                                 self.pos[act.id].1 - self.y_off);
                },
                PressAction::Press => {
                    if self.pos[act.id].1 >= self.y_off {
                        self.btn.press(act.id,
                                       self.pos[act.id].0,
                                       self.pos[act.id].1 - self.y_off);
                    }
                },
                PressAction::Release => {
                    self.btn.release(act.id);
                },
            }
        }

        self.actions.clear();
    }

    pub fn handle_events(&mut self)
    {
        let evts: Vec<_> = self.dev.fetch_events().unwrap().collect();

        for evt in evts {
            match evt.destructure() {
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_SLOT, val) => {
                    self.slot = val as usize;
                },
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_POSITION_X, val) => {
                    self.pos[self.slot].0 = val as f64;
                    let action = TouchAction {
                        id: self.slot,
                        act: PressAction::Pos,
                    };
                    self.actions.push(action);
                },
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_POSITION_Y, val) => {
                    self.pos[self.slot].1 = val as f64;
                    let action = TouchAction {
                        id: self.slot,
                        act: PressAction::Pos,
                    };
                    self.actions.push(action);
                },
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_TRACKING_ID, -1) => {
                    let action = TouchAction {
                        id: self.slot,
                        act: PressAction::Release,
                    };
                    self.actions.push(action);
                },
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_TRACKING_ID, _) => {
                    let action = TouchAction {
                        id: self.slot,
                        act: PressAction::Press,
                    };
                    self.actions.push(action);
                },
                EventSummary::Synchronization(_, _, _) => {
                    self.commit();
                },
                _ => (),
            }
        }
    }
}

impl<D: Display, K: Keyboard> AsFd for Touchscreen<D, K> {
    fn as_fd<'a>(&'a self) -> BorrowedFd<'a>
    {
        self.dev.as_fd()
    }
}

impl<D: Display, K: Keyboard> AsRawFd for Touchscreen<D, K> {
    fn as_raw_fd(&self) -> RawFd
    {
        self.dev.as_raw_fd()
    }
}
