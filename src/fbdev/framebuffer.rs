// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2025, Unfettered Keyboard Contributors
 */

use crate::core::Configuration;
use crate::core::Display;
use crate::fbdev::linuxfb;
use imgref::ImgRefMut;
use rgb::FromSlice;
use rgb::alt::BGRA;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Error;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::os::fd::AsRawFd;

pub struct Framebuffer {
    f: File,
    y: u32,
    size: (u32, u32),
    stride: u32,
    img: Vec<u8>,
}

impl Framebuffer {
    pub fn new(cfg: &Configuration) -> Result<Self, Error>
    {
        let mut open = OpenOptions::new();
        open.read(true);
        open.write(true);
        let f = open.open("/dev/fb0")?;

        let mut finfo = linuxfb::fb_fix_screeninfo {
            id: [0; 16],
            smem_start: 0,
            smem_len: 0,
            type_: 0,
            type_aux: 0,
            visual: 0,
            xpanstep: 0,
            ypanstep: 0,
            ywrapstep: 0,
            line_length: 0,
            mmio_start: 0,
            mmio_len: 0,
            accel: 0,
            capabilities: 0,
            reserved: [0; 2],
        };

        let ret = unsafe { libc::ioctl(f.as_raw_fd(),
                                       linuxfb::FBIOGET_FSCREENINFO as libc::Ioctl,
                                       &mut finfo as *mut _) };
        if ret == -1 {
            return Err(Error::last_os_error());
        }

        let mut vinfo = linuxfb::fb_var_screeninfo {
            xres: 0,
            yres: 0,
            xres_virtual: 0,
            yres_virtual: 0,
            xoffset: 0,
            yoffset: 0,
            bits_per_pixel: 0,
            grayscale: 0,
            red: linuxfb::fb_bitfield { offset: 0, length: 0, msb_right: 0 },
            green: linuxfb::fb_bitfield { offset: 0, length: 0, msb_right: 0 },
            blue: linuxfb::fb_bitfield { offset: 0, length: 0, msb_right: 0 },
            transp: linuxfb::fb_bitfield { offset: 0, length: 0, msb_right: 0 },
            nonstd: 0,
            activate: 0,
            height: 0,
            width: 0,
            accel_flags: 0,
            pixclock: 0,
            left_margin: 0,
            right_margin: 0,
            upper_margin: 0,
            lower_margin: 0,
            hsync_len: 0,
            vsync_len: 0,
            sync: 0,
            vmode: 0,
            rotate: 0,
            colorspace: 0,
            reserved: [0; 4],
        };

        let ret = unsafe { libc::ioctl(f.as_raw_fd(),
                                       linuxfb::FBIOGET_VSCREENINFO as libc::Ioctl,
                                       &mut vinfo as *mut _) };
        if ret == -1 {
            return Err(Error::last_os_error());
        }

        let height = (vinfo.yres_virtual as f64 * cfg.evfb_height()) as u32;
        let y = vinfo.yres_virtual - height;

        let size = (vinfo.xres_virtual, height as u32);

        Ok(Self {
            f,
            y,
            size,
            stride: finfo.line_length,
            img: vec![0; size.0 as usize * finfo.line_length as usize * 4],
        })
    }
}

impl Display for Framebuffer {
    fn size(&self) -> (u32, u32)
    {
        self.size
    }

    fn begin<'a>(&'a mut self) -> ImgRefMut<'a, BGRA<u8>>
    {
        // The ARGB format is in little endian.
        let ptr = &mut self.img;
        let ptr = ptr.as_bgra_mut();
        let img = ImgRefMut::new_stride(ptr,
                                        self.size.0 as usize,
                                        self.size.1 as usize,
                                        (self.stride / 4) as usize);

        img
    }

    fn resize(&mut self, _width: u32, _height: u32)
    {
        // Nothing to do here, we resize once on startup and everything is working.
    }

    fn end(&mut self, _x: u32, y: u32, _width: u32, height: u32)
    {
        let kbd_top = self.y as u64 * self.stride as u64;
        let target_top = y as usize * self.stride as usize;
        let target_bottom = (y as usize + height as usize) * self.stride as usize;
        let subimg = &self.img[target_top..target_bottom];

        self.f.seek(SeekFrom::Start(kbd_top + target_top as u64)).unwrap();
        self.f.write(subimg).unwrap();
    }
}
