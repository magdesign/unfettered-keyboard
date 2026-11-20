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
use std::io::ErrorKind;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;

const BYTES_PER_PIXEL: u32 = 4;

pub struct Framebuffer {
    f: File,
    // Byte offset of the keyboard's top-left pixel in the framebuffer device.
    offset: u64,
    size: (u32, u32),
    // Bytes per line, in both the device and the local image.
    stride: u32,
    // Whether the visible area starts at the beginning of each line.
    full_lines: bool,
    img: Vec<u8>,
}

fn invalid(msg: String) -> Error
{
    Error::new(ErrorKind::InvalidData, msg)
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

        // Pixels are drawn as 32-bit BGRA; other formats would be garbled.
        if vinfo.bits_per_pixel != BYTES_PER_PIXEL * 8 {
            return Err(Error::new(ErrorKind::Unsupported,
                                  format!("framebuffer has {} bits per pixel, only 32 is supported",
                                          vinfo.bits_per_pixel)));
        }

        let stride = finfo.line_length;
        if stride % BYTES_PER_PIXEL != 0
        || (stride as u64) < vinfo.xres as u64 * BYTES_PER_PIXEL as u64 {
            return Err(invalid(format!("framebuffer line length {} is too short for {} pixels",
                                       stride, vinfo.xres)));
        }

        /*
         * The visible screen is the xres x yres area at (xoffset, yoffset) of
         * the virtual framebuffer, which may be larger (e.g. double buffering).
         */
        if vinfo.xres == 0 || vinfo.yres == 0
        || vinfo.xoffset as u64 + vinfo.xres as u64 > vinfo.xres_virtual as u64
        || vinfo.yoffset as u64 + vinfo.yres as u64 > vinfo.yres_virtual as u64 {
            return Err(invalid(format!("invalid framebuffer geometry {}x{}+{}+{} in {}x{}",
                                       vinfo.xres, vinfo.yres,
                                       vinfo.xoffset, vinfo.yoffset,
                                       vinfo.xres_virtual, vinfo.yres_virtual)));
        }

        let height = (vinfo.yres as f64 * cfg.evfb_height()) as u32;
        let height = height.clamp(1, vinfo.yres);
        let top = vinfo.yoffset + vinfo.yres - height;

        let offset = top as u64 * stride as u64
                   + vinfo.xoffset as u64 * BYTES_PER_PIXEL as u64;

        Ok(Self {
            f,
            offset,
            size: (vinfo.xres, height),
            stride,
            full_lines: vinfo.xoffset == 0,
            img: vec![0; height as usize * stride as usize],
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
                                        (self.stride / BYTES_PER_PIXEL) as usize);

        img
    }

    fn resize(&mut self, _width: u32, _height: u32)
    {
        // Nothing to do here, we resize once on startup and everything is working.
    }

    fn end(&mut self, _x: u32, y: u32, _width: u32, height: u32)
    {
        // Only write rows that exist, even if asked for more.
        let y = y.min(self.size.1);
        let height = height.min(self.size.1 - y);
        let stride = self.stride as usize;

        let res = if self.full_lines {
            // Whole lines are contiguous in both buffers: write them at once.
            let start = y as usize * stride;
            let end = start + height as usize * stride;
            self.f.write_all_at(&self.img[start..end], self.offset + start as u64)
        } else {
            // Write each line's visible part, leaving the rest of the line alone.
            let width = self.size.0 as usize * BYTES_PER_PIXEL as usize;
            (y as usize..(y + height) as usize).try_for_each(|row| {
                let start = row * stride;
                self.f.write_all_at(&self.img[start..start + width],
                                    self.offset + start as u64)
            })
        };

        if let Err(e) = res {
            eprintln!("warn: failed to write to the framebuffer: {}", e);
        }
    }
}
