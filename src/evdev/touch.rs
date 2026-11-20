// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

use crate::core::Button;
use crate::core::Configuration;
use crate::core::Display;
use crate::core::Keyboard;
use evdev::AbsoluteAxisCode;
use evdev::EventSummary;
use evdev::InputEvent;
use evdev::SynchronizationCode;
use evdev::raw_stream::RawDevice;
use std::convert::TryFrom;
use std::fs;
use std::io::Error;
use std::io::ErrorKind;
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
    /*
     * The raw device is used because it reports SYN_DROPPED. The synced
     * device compensates dropped events, but not for multitouch slots, so a
     * lifted finger could be lost and its key would repeat forever.
     */
    dev: RawDevice,
    btn: Button<D, K>,
    actions: Vec<TouchAction>,
    pos: Vec<(f64, f64)>,
    // The current slot, or None if the device reported an invalid one.
    slot: Option<usize>,
    y_off: f64,
    // Events are being discarded until the next SYN_REPORT after SYN_DROPPED.
    dropping: bool,
}

fn set_nonblocking(fd: RawFd) -> Result<(), Error>
{
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(Error::last_os_error());
    }

    let ret = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if ret == -1 {
        return Err(Error::last_os_error());
    }

    Ok(())
}

impl<D: Display, K: Keyboard> Touchscreen<D, K> {
    fn open_device_if_supported(dent: Result<fs::DirEntry, Error>) -> Option<RawDevice>
    {
        let path = dent.ok()?.path();
        let dev = RawDevice::open(path).ok()?;

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
        let dev = devs.find_map(Self::open_device_if_supported)
                      .ok_or(Error::new(ErrorKind::NotFound,
                                        "no multitouch touchscreen found in /dev/input"))?;

        set_nonblocking(dev.as_raw_fd())?;

        let props = dev.get_abs_state()?;
        let slot_info = props[AbsoluteAxisCode::ABS_MT_SLOT.0 as usize];
        let max_slots = usize::try_from(slot_info.maximum).unwrap_or(0) + 1;
        let slot = usize::try_from(slot_info.value).ok().filter(|&s| s < max_slots);

        // Dimensions of the touchscreen device
        let d_width = props[AbsoluteAxisCode::ABS_MT_POSITION_X.0 as usize].maximum;
        let d_width = d_width as f64 + 1.0;
        let d_height = props[AbsoluteAxisCode::ABS_MT_POSITION_Y.0 as usize].maximum;
        let d_height = d_height as f64 + 1.0;

        if d_width <= 0.0 || d_height <= 0.0 {
            return Err(Error::new(ErrorKind::InvalidData,
                                  "touchscreen reports an empty touch area"));
        }

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
            slot,
            y_off: d_height - c_height,
            dropping: false,
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
            let (x, y) = match self.pos.get(act.id) {
                Some(&p) => p,
                None => continue,
            };

            match act.act {
                PressAction::Pos => {
                    self.btn.pos(act.id, x, y - self.y_off);
                },
                PressAction::Press => {
                    if y >= self.y_off {
                        self.btn.press(act.id, x, y - self.y_off);
                    }
                },
                PressAction::Release => {
                    self.btn.release(act.id);
                },
            }
        }

        self.actions.clear();
    }

    /*
     * Some events were lost, possibly including lifted fingers. Forget every
     * press without emitting anything. Fingers that are still down are ignored
     * until they are lifted, as their presses no longer exist.
     */
    fn drop_events(&mut self)
    {
        self.actions.clear();
        self.btn.cancel_all();
        self.dropping = true;
    }

    // Re-read the state that the lost events may have changed.
    fn resync(&mut self)
    {
        match self.dev.get_abs_state() {
            Ok(props) => {
                let val = props[AbsoluteAxisCode::ABS_MT_SLOT.0 as usize].value;
                self.slot = usize::try_from(val).ok().filter(|&s| s < self.pos.len());
            },
            Err(e) => {
                eprintln!("warn: failed to resync touchscreen state: {}", e);
                self.slot = None;
            },
        }
    }

    fn push_action(&mut self, act: PressAction)
    {
        if let Some(id) = self.slot {
            self.actions.push(TouchAction { id, act });
        }
    }

    /*
     * Process the available events. An error means the touchscreen can no
     * longer be read (e.g. it was removed).
     */
    pub fn handle_events(&mut self) -> Result<(), Error>
    {
        let evts: Vec<InputEvent> = match self.dev.fetch_events() {
            Ok(evts) => evts.collect(),
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(e),
        };

        for evt in evts {
            let evt = evt.destructure();

            if self.dropping {
                // Discard everything up to and including the next report.
                if let EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) = evt {
                    self.dropping = false;
                    self.resync();
                }
                continue;
            }

            match evt {
                EventSummary::Synchronization(_, SynchronizationCode::SYN_DROPPED, _) => {
                    self.drop_events();
                },
                EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
                    self.commit();
                },
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_SLOT, val) => {
                    self.slot = usize::try_from(val).ok().filter(|&s| s < self.pos.len());
                },
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_POSITION_X, val) => {
                    if let Some(slot) = self.slot {
                        self.pos[slot].0 = val as f64;
                        self.push_action(PressAction::Pos);
                    }
                },
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_POSITION_Y, val) => {
                    if let Some(slot) = self.slot {
                        self.pos[slot].1 = val as f64;
                        self.push_action(PressAction::Pos);
                    }
                },
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_TRACKING_ID, -1) => {
                    self.push_action(PressAction::Release);
                },
                EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_TRACKING_ID, _) => {
                    self.push_action(PressAction::Press);
                },
                _ => (),
            }
        }

        Ok(())
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
