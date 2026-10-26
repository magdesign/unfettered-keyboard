// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

use futures_util::stream::StreamExt;
use std::convert::TryFrom;
use std::env;
use std::sync::Arc;
use tokio::task;
use zbus::Connection;
use zbus::Result;
use zbus::fdo::DBusProxy;
use zbus::names::BusName;
use zbus::proxy;
use zbus::zvariant::OwnedObjectPath;

#[proxy(interface = "org.gnome.SessionManager",
        default_service = "org.gnome.SessionManager",
        default_path = "/org/gnome/SessionManager")]
trait SessionManager {
    fn register_client(&self, app_id: &str, client_startup_id: &str) -> Result<OwnedObjectPath>;
}

#[proxy(interface = "org.gnome.SessionManager.ClientPrivate")]
trait ClientPrivate {
    fn end_session_response(&self, is_ok: bool, reason: &str) -> Result<()>;

    #[zbus(signal)]
    fn query_end_session(&self, flags: u32);
}

pub async fn register(conn: &Connection) -> Result<()>
{
    let manager = SessionManagerProxy::new(conn).await?;

    let autostart_id = env::var("DESKTOP_AUTOSTART_ID").unwrap_or(String::new());
    let path = manager.register_client("sm.puri.OSK0", &autostart_id).await?;

    let dbus = DBusProxy::new(conn).await?;
    let name = BusName::try_from("org.gnome.SessionManager")?;
    let id = dbus.get_name_owner(name).await?;

    let client_priv = ClientPrivateProxy::new(conn, id, path).await?;
    let client_priv = Arc::new(client_priv);

    {
        let client_priv = client_priv.clone();
        let mut stream_qes = client_priv.receive_query_end_session().await?;

        task::spawn(async move {
            while stream_qes.next().await.is_some() {
                let _ = client_priv.end_session_response(true, "").await;
            }
        });
    }

    {
        let client_priv = client_priv.clone();
        let mut stream_es = client_priv.receive_query_end_session().await?;

        task::spawn(async move {
            while stream_es.next().await.is_some() {
                let _ = client_priv.end_session_response(true, "").await;
            }
        });
    }

    Ok(())
}
