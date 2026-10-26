// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

mod core;
mod evdev;
mod wayland;

use crate::core::Configuration;
use crate::core::Polled;
use crate::wayland::Dispatcher;
use polling::Event;
use polling::Events;
use polling::Poller;
use std::process::ExitCode;
use std::time::Instant;
use wayland_client::globals;

fn main() -> ExitCode
{
    let conn = wayland_client::Connection::connect_to_env().unwrap();

    let (globals, mut queue) = globals::registry_queue_init::<Dispatcher>(&conn)
                                            .expect("Registry required");

    let config = match Configuration::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::FAILURE;
        },
    };

    let mut dispatcher = Dispatcher::new(&config, queue.handle(), &globals).unwrap();

    let wl_evt = Event::readable(0);

    let mut events = Events::new();
    let poller = Poller::new().unwrap();

    loop {
        conn.flush().unwrap();

        let guard = queue.prepare_read().unwrap();
        let fd = guard.connection_fd();
        let timer = dispatcher.button().next_time().map(|t| t - Instant::now());

        {
            let _wl_poll = Polled::add(&poller, &fd, wl_evt).unwrap();
            events.clear();
            poller.wait(&mut events, timer).unwrap();
        }

        if !events.is_empty() {
            // This may still emit EWOULDBLOCK errors.
            let _ = guard.read();
            queue.dispatch_pending(&mut dispatcher).unwrap();
        }

        dispatcher.dispatch_timers();
    }
}
