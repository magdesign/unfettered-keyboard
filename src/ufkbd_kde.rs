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

    let mut dispatcher = match Dispatcher::new(&config, queue.handle(), &globals) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{:?}", e);
            return ExitCode::FAILURE;
        },
    };

    let wl_evt = Event::readable(0);

    let mut events = Events::new();
    let poller = match Poller::new() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("creating poller: {}", e);
            return ExitCode::FAILURE;
        },
    };

    // This shows a permission dialog, which the user may deny.
    let ei_ctx = match connect_to_eis().await {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("could not get keyboard access from the remote desktop portal: {}", e);
            return ExitCode::FAILURE;
        },
    };

    let ei_evt = Event::readable(1);

    // Haptics are optional: feedbackd is not available everywhere.
    match zbus::Connection::session().await {
        Ok(conn_zbus) => match FeedbackClient::new(&conn_zbus).await {
            Ok(haptics) => dispatcher.set_haptics(Box::new(haptics)),
            Err(e) => eprintln!("warn: haptic feedback unavailable: {}", e),
        },
        Err(e) => eprintln!("warn: haptic feedback unavailable: {}", e),
    }

    let res = task::spawn_blocking(move || {
        if let Err(e) = handshake::ei_handshake_blocking(&ei_ctx, "ufkbd-kde",
                                                         ContextType::Sender) {
            eprintln!("ei handshake failed: {:?}", e);
            return ExitCode::FAILURE;
        }

        loop {
            if let Err(e) = conn.flush() {
                eprintln!("lost connection to the compositor: {}", e);
                return ExitCode::FAILURE;
            }
            if let Err(e) = ei_ctx.flush() {
                eprintln!("lost ei connection: {}", e);
                return ExitCode::FAILURE;
            }

            let guard = match queue.prepare_read() {
                Some(g) => g,
                None => {
                    // Events are already queued; dispatch them first.
                    if let Err(e) = queue.dispatch_pending(&mut dispatcher) {
                        eprintln!("dispatching Wayland events: {}", e);
                        return ExitCode::FAILURE;
                    }
                    continue;
                },
            };
            let wlfd = guard.connection_fd();
            let timer = dispatcher.button().next_time().map(|t| t - Instant::now());

            {
                let _wl_poll = Polled::add(&poller, &wlfd, wl_evt).unwrap();
                let _ei_poll = Polled::add(&poller, &ei_ctx, ei_evt).unwrap();
                events.clear();
                poller.wait(&mut events, timer).unwrap();
            }

            if !events.is_empty() {
                // This may still emit EWOULDBLOCK errors.
                let _ = guard.read();

                // The ei socket reports a closed connection as an error.
                if let Err(e) = ei_ctx.read() {
                    eprintln!("lost ei connection: {}", e);
                    return ExitCode::FAILURE;
                }

                if let Err(e) = queue.dispatch_pending(&mut dispatcher) {
                    eprintln!("dispatching Wayland events: {}", e);
                    return ExitCode::FAILURE;
                }

                if let Err(e) = dispatcher.dispatch_ei(&ei_ctx) {
                    eprintln!("ei: {}", e);
                    return ExitCode::FAILURE;
                }
            }

            dispatcher.dispatch_timers();
        }
    }).await;

    res.unwrap_or(ExitCode::FAILURE)
}

async fn connect_to_eis() -> Result<Context, Box<dyn std::error::Error>>
{
    let rd = RemoteDesktop::new().await?;
    let session = rd.create_session(CreateSessionOptions::default()).await?;
    let select_devices = SelectDevicesOptions::default()
                            .set_devices(Some(DeviceType::Keyboard.into()))
                            .set_persist_mode(PersistMode::DoNot);
    rd.select_devices(&session, select_devices).await?;
    // Fails if the user denies the request.
    rd.start(&session, None, StartOptions::default()).await?.response()?;
    let eifd = rd.connect_to_eis(&session, ConnectToEISOptions::default()).await?;
    Ok(Context::new(eifd.into())?)
}
