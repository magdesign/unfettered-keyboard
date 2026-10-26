// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2026, Unfettered Keyboard Contributors
 */

use polling::AsSource;
use polling::AsRawSource;
use polling::Event;
use polling::Poller;
use std::io::Error;

pub struct Polled<'a> {
    poller: &'a Poller,
    source: &'a dyn AsSource,
}

impl<'a> Polled<'a> {
    pub fn add<T>(poller: &'a Poller, source: &'a T, interest: Event)
               -> Result<Self, Error>
              where T: AsSource, &'a T: AsRawSource
    {
        unsafe { poller.add(source, interest) }?;
        Ok(Self { poller, source })
    }
}

impl Drop for Polled<'_> {
    fn drop(&mut self)
    {
        let _ = self.poller.delete(self.source);
    }
}
