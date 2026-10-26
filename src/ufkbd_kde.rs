// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

mod core;
mod dbus;
mod evdev;
mod wayland;

use ashpd::desktop::CreateSessionOptions;
use ashpd::desktop::PersistMode;
use ashpd::desktop::remote_desktop::ConnectToEISOptions;
use ashpd::desktop::remote_desktop::StartOptions;
use ashpd::desktop::remote_desktop::DeviceType;
use ashpd::desktop::remote_desktop::RemoteDesktop;
use ashpd::desktop::remote_desktop::SelectDevicesOptions;
use crate::core::Configuration;
use crate::core::Polled;
use crate::dbus::feedbackd::FeedbackClient;
use crate::wayland::Dispatcher;
use polling::Event;
use polling::Events;
use polling::Poller;
use std::process::ExitCode;
use std::time::Instant;
use reis::ei::Context;
use reis::ei::handshake::ContextType;
use reis::handshake;
use tokio::task;
use wayland_client::globals;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode
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

    let rd = RemoteDesktop::new().await.unwrap();
    let session = rd.create_session(CreateSessionOptions::default()).await.unwrap();
    let select_devices = SelectDevicesOptions::default()
                            .set_devices(Some(DeviceType::Keyboard.into()))
                            .set_persist_mode(PersistMode::DoNot);
    rd.select_devices(&session, select_devices).await.unwrap();
    rd.start(&session, None, StartOptions::default()).await.unwrap();
    let eifd = rd.connect_to_eis(&session, ConnectToEISOptions::default()).await.unwrap();
    let ei_ctx = Context::new(eifd.into()).unwrap();

    let ei_evt = Event::readable(1);

    let conn_zbus = zbus::Connection::session().await.unwrap();
    let haptics = Box::new(FeedbackClient::new(&conn_zbus).await.unwrap());
    dispatcher.set_haptics(haptics);

    task::spawn_blocking(move || {
        handshake::ei_handshake_blocking(&ei_ctx, "ufkbd-kde",
                                         ContextType::Sender).unwrap();

        loop {
            conn.flush().unwrap();
            ei_ctx.flush().unwrap();

            let guard = queue.prepare_read().unwrap();
            let wlfd = guard.connection_fd();
            let timer = dispatcher.button().next_time().map(|t| t - Instant::now());

            {
                let _wl_poll = Polled::add(&poller, &wlfd, wl_evt).unwrap();
                let _ei_poll = Polled::add(&poller, &ei_ctx, ei_evt).unwrap();
                events.clear();
                poller.wait(&mut events, timer).unwrap();
            }

            if !events.is_empty() {
                let _ = guard.read();
                let _ = ei_ctx.read();

                queue.dispatch_pending(&mut dispatcher).unwrap();
                dispatcher.dispatch_ei(&ei_ctx);
            }

            dispatcher.dispatch_timers();
        }
    }).await.unwrap();

    ExitCode::SUCCESS
}
