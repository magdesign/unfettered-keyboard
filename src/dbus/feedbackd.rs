// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2026, Unfettered Keyboard Contributors
 */

use crate::core::Haptics;
use crate::core::FeedbackType;
use std::collections::HashMap;
use tokio::task;
use zbus::Connection;
use zbus::Result;
use zbus::proxy;
use zbus::zvariant::Value;

#[proxy(interface = "org.sigxcpu.Feedback",
        default_service = "org.sigxcpu.Feedback",
        default_path = "/org/sigxcpu/Feedback")]
trait Feedback {
    fn trigger_feedback(&self, app_id: &str, event: &str,
                        hints: &HashMap<String, Value<'_>>, timeout: i32)
                     -> Result<u32>;
}

pub struct FeedbackClient<'p> {
    proxy: FeedbackProxy<'p>,
}

impl<'p> FeedbackClient<'p> {
    pub async fn new(conn: &Connection) -> Result<Self>
    {
        let proxy = FeedbackProxy::new(conn).await?;

        Ok(Self {
            proxy,
        })
    }
}

impl Haptics for FeedbackClient<'static> {
    fn vibrate(&self, feedback_type: FeedbackType)
    {
        let event = match feedback_type {
            FeedbackType::GestureStart => "key-pressed",
            FeedbackType::GestureEnd => "key-released",
            FeedbackType::KeyRepeat => "key-released",
        };

        let proxy = self.proxy.clone();

        task::spawn(async move {
            let ret = proxy.trigger_feedback("unfettered.keyboard",
                                             event,
                                             &HashMap::new(),
                                             -1).await;
            if ret.is_err() {
                eprintln!("warn: failed to trigger haptics");
            }
        });
    }
}

