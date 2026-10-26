// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

use crate::core::Configuration;
use crate::core::Display;
use crate::wayland::Buffer;
use crate::wayland::Dispatcher;
use crate::wayland::fractional_scale_v1::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use crate::wayland::viewporter::wp_viewporter::WpViewporter;
use crate::wayland::viewporter::wp_viewport::WpViewport;
use crate::wayland::wlr_layer_shell_unstable_v1::zwlr_layer_shell_v1;
use crate::wayland::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1;
use imgref::ImgRefMut;
use rgb::alt::BGRA;
use std::io::Error;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::QueueHandle;

pub struct Surface {
    bufs: [Buffer; 2],

    queue: QueueHandle<Dispatcher>,
    comp: WlCompositor,
    layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1,
    vper: Option<WpViewporter>,
    fsm: Option<WpFractionalScaleManagerV1>,

    surface: Option<WlSurface>,
    layer_surf: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    vp: Option<WpViewport>,

    used_buf: usize,
    configured: bool,
    scale: u32,
    height: i32,
}

const LAYER_ANCHOR: zwlr_layer_surface_v1::Anchor
                   = zwlr_layer_surface_v1::Anchor::from_bits_truncate(0)
                         .union(zwlr_layer_surface_v1::Anchor::Bottom)
                         .union(zwlr_layer_surface_v1::Anchor::Left)
                         .union(zwlr_layer_surface_v1::Anchor::Right);

impl Surface {
    pub fn new(cfg: &Configuration, queue: QueueHandle<Dispatcher>,
                   shm: &WlShm, comp: WlCompositor,
                   layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1,
                   fsm: Option<WpFractionalScaleManagerV1>,
                   vper: Option<WpViewporter>)
                -> Result<Self, Error>
    {
        let buf1 = Buffer::new(queue.clone(), shm, 0)?;
        let buf2 = Buffer::new(queue.clone(), shm, 1)?;

        let height = cfg.wayland_height();

        Ok(Self {
            bufs: [buf1, buf2],

            queue: queue.clone(),
            comp,
            layer_shell,
            vper,
            fsm,

            surface: None,
            layer_surf: None,
            vp: None,

            used_buf: 1,
            configured: false,
            scale: 120,
            height,
        })
    }

    #[inline(always)]
    pub fn configured(&self) -> bool
    {
        self.configured
    }

    #[inline(always)]
    pub fn scale(&self) -> u32
    {
        self.scale
    }

    pub fn ack_configure(&mut self, serial: u32)
    {
        self.configured = true;
        self.layer_surf.as_ref().unwrap().ack_configure(serial);
    }

    pub fn update_scale(&mut self, scale: u32)
    {
        self.scale = scale;
    }

    pub fn release(&mut self, id: u32)
    {
        self.bufs[id as usize].release();
    }

    pub fn hide(&mut self)
    {
        self.configured = false;

        if let Some(vp) = &self.vp {
            vp.destroy();
            self.vp = None;
        }

        if let Some(ls) = &self.layer_surf {
            ls.destroy();
            self.layer_surf = None;
        }

        if let Some(s) = &self.surface {
            s.destroy();
            self.surface = None;
        }
    }

    pub fn show(&mut self)
    {
        if self.layer_surf.is_some() || self.surface.is_some() {
            return;
        }

        let surface = self.comp.create_surface(&self.queue, ());

        let layer_surf = self.layer_shell.get_layer_surface(&surface, None,
                                                            zwlr_layer_shell_v1::Layer::Top,
                                                            String::from("osk"),
                                                            &self.queue, ());

        self.fsm.as_ref().map(|fsm| {
            fsm.get_fractional_scale(&surface, &self.queue, ())
        });

        let vp = self.vper.as_ref().map(|vper| {
            vper.get_viewport(&surface, &self.queue, ())
        });

        layer_surf.set_size(0, self.height as u32);
        layer_surf.set_exclusive_zone(self.height);
        layer_surf.set_anchor(LAYER_ANCHOR);

        surface.commit();

        self.surface = Some(surface);
        self.layer_surf = Some(layer_surf);
        self.vp = vp;
    }
}

impl Display for Surface {
    #[inline(always)]
    fn size(&self) -> (u32, u32)
    {
        let (width, height) = self.bufs[self.used_buf].size();
        (width as u32, height as u32)
    }

    fn begin<'a>(&'a mut self) -> ImgRefMut<'a, BGRA<u8>>
    {
        for (id, buf) in self.bufs.iter_mut().enumerate() {
            if let Some(img) = buf.consume(self.surface.as_ref()) {
                self.used_buf = id;
                return img;
            }
        }

        panic!("No buffers available")
    }

    fn resize(&mut self, width: u32, height: u32)
    {
        let width_unscaled = (width * 120 / self.scale) as i32;
        let height_unscaled = (height * 120 / self.scale) as i32;

        for buf in &mut self.bufs {
            buf.resize(width as i32, height as i32).unwrap();
        }

        if let Some(vp) = &self.vp {
            vp.set_source(0.0, 0.0, width as f64, height as f64);
            vp.set_destination(width_unscaled, height_unscaled);
        }
    }

    fn end(&mut self, x: u32, y: u32, width: u32, height: u32)
    {
        if let Some(surface) = &self.surface {
            surface.damage_buffer(x as i32, y as i32, width as i32, height as i32);
            surface.commit();
        }

        for (id, buf) in self.bufs.iter_mut().enumerate() {
            if id != self.used_buf {
                buf.release();
            }
        }

        let (src, dest) = match self.used_buf {
            0 => {
                let [src, ref mut dest] = &mut self.bufs;
                (src, dest)
            },
            1 => {
                let [ref mut dest, src] = &mut self.bufs;
                (src, dest)
            },
            2.. => panic!("Surface must only use two buffers"),
        };

        let img = src.image().sub_image(x as usize,
                                        y as usize,
                                        width as usize,
                                        height as usize);
        dest.writeback(&img, x as usize, y as usize);
    }
}
