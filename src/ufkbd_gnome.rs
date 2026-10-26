// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

mod core;
mod dbus;
mod evdev;
mod wayland;

use crate::core::Configuration;
use crate::core::Polled;
use crate::dbus::session;
use crate::dbus::osk::OSK0;
use crate::wayland::Dispatcher;
use crate::wayland::VisibilityToggle;
use polling::Event;
use polling::Events;
use polling::Poller;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;
use tokio::task;
use wayland_client::globals;
use zbus::object_server::InterfaceRef;

pub struct VisibilityManager(InterfaceRef<OSK0>);

impl VisibilityToggle for VisibilityManager {
    fn set_visible(&self, visible: bool)
    {
        let osk = self.0.clone();

        task::spawn(async move {
            let osk = osk.get().await;
            osk.set_visible(visible).await;
        });
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode
{
    let conn_wl = wayland_client::Connection::connect_to_env().unwrap();

    let (globals, mut queue) = globals::registry_queue_init::<Dispatcher>(&conn_wl)
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

    let conn_zbus = zbus::Connection::session().await.unwrap();

    let res = session::register(&conn_zbus).await;
    if let Err(e) = res {
        eprintln!("warn: GNOME session registration failed: {}", e);
    }

    let conn_wl = Arc::new(conn_wl);

    let gfx = dispatcher.graphics();
    let osk = OSK0::start(&conn_zbus, gfx, conn_wl.clone()).await;
    match osk {
        Ok(k) => dispatcher.set_osk(Box::new(VisibilityManager(k))),
        Err(e) => eprintln!("warn: bind to sm.puri.OSK0 failed: {}", e),
    }

    let mut events = Events::new();
    let poller = Poller::new().unwrap();

    task::spawn_blocking(move || {
        loop {
            conn_wl.flush().unwrap();

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
    }).await.unwrap();

    ExitCode::SUCCESS
}
