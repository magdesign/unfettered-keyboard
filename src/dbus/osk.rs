// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

use crate::core::Graphics;
use crate::wayland::Surface;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use zbus::Connection;
use zbus::Result;
use zbus::interface;
use zbus::object_server::InterfaceRef;
use zbus::object_server::SignalEmitter;

pub struct OSK0 {
    visible: AtomicBool,
    gfx: Arc<Mutex<Graphics<Surface>>>,
    sigctx: SignalEmitter<'static>,
    conn_wl: Arc<wayland_client::Connection>,
}

#[interface(name = "sm.puri.OSK0")]
impl OSK0 {
    fn get_visible(&self) -> bool
    {
        self.visible.load(Ordering::Relaxed)
    }

    pub async fn set_visible(&self, visible: bool)
    {
        self.visible.store(visible, Ordering::Relaxed);
        self.visible_changed(&self.sigctx).await.unwrap();

        let mut gfx = self.gfx.lock().unwrap();
        let disp = gfx.display_mut();
        if visible {
            disp.show();
        } else {
            disp.hide();
        }

        self.conn_wl.flush().unwrap();
    }

    #[zbus(property)]
    fn visible(&self) -> bool
    {
        self.visible.load(Ordering::Relaxed)
    }
}

impl OSK0 {
    pub async fn start(conn: &Connection,
                       gfx: Arc<Mutex<Graphics<Surface>>>,
                       conn_wl: Arc<wayland_client::Connection>)
                    -> Result<InterfaceRef<Self>>
    {
        let sigctx = SignalEmitter::new(conn, "/sm/puri/OSK0")?;

        let visible = AtomicBool::new(false);
        let osk = OSK0 { visible, gfx, sigctx, conn_wl };

        let server = conn.object_server();
        server.at("/sm/puri/OSK0", osk).await?;
        conn.request_name("sm.puri.OSK0").await?;

        let osk = server.interface("/sm/puri/OSK0").await?;

        Ok(osk)
    }
}
