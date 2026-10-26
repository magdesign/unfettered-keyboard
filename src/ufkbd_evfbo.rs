// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2025, Unfettered Keyboard Contributors
 */

mod core;
mod evdev;
mod fbdev;
mod stdio;

use crate::core::Button;
use crate::core::Configuration;
use crate::core::Display;
use crate::core::Graphics;
use crate::core::Layout;
use crate::core::Polled;
use crate::evdev::Touchscreen;
use crate::fbdev::Framebuffer;
use crate::stdio::Output;
use polling::Event;
use polling::Events;
use polling::Poller;
use signal_hook::consts::signal;
use signal_hook::iterator::Signals;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

fn main() -> ExitCode
{
    let cfg = match Configuration::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::FAILURE;
        },
    };
    let disp = Framebuffer::new(&cfg).unwrap();

    let gfx = Graphics::new(disp, &cfg);
    let gfx = Mutex::new(gfx);
    let gfx = Arc::new(gfx);

    let kbd = Output::new();
    let layout = Layout::load(&cfg).unwrap();
    let mut btn = Button::new(&cfg, layout, kbd, gfx.clone());
    btn.set_text_supported(true);

    {
        let mut gfx = gfx.lock().unwrap();
        let (width, height) = gfx.display().size();
        gfx.resize(btn.layout(), btn.mod_state(), width, height);
    }

    let mut touchscreen = Touchscreen::new(btn, &cfg).unwrap();

    let mut events = Events::new();
    let poller = Poller::new().unwrap();
    let ts_evt = Event::readable(0);

    let mut sigs = Signals::new([
        signal::SIGHUP,
        signal::SIGINT,
        signal::SIGPIPE,
        signal::SIGTERM,
    ]).unwrap();

    while sigs.pending().next().is_none() && !touchscreen.button().keyboard().complete() {
        let timer = touchscreen.button().next_time().map(|t| t - Instant::now());

        {
            let _ts_poll = Polled::add(&poller, &touchscreen, ts_evt).unwrap();
            events.clear();
            poller.wait(&mut events, timer).unwrap();
        }

        if !events.is_empty() {
            touchscreen.handle_events();
        }

        touchscreen.dispatch_timers();
    }

    ExitCode::SUCCESS
}
