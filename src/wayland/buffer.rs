// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

use crate::wayland::Dispatcher;
use imgref::ImgRef;
use imgref::ImgRefMut;
use memmap2::MmapMut;
use rgb::alt::BGRA;
use rgb::FromSlice;
use std::ffi::CString;
use std::fs::File;
use std::io::Error;
use std::io::ErrorKind;
use std::iter;
use std::os::fd::FromRawFd;
use std::os::fd::AsFd;
use std::process;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_shm;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::QueueHandle;

pub struct Buffer {
    file: File,
    map: MmapMut,

    id: u32,
    capacity: i32,
    width: i32,
    height: i32,

    queue: QueueHandle<Dispatcher>,
    pool: WlShmPool,
    buf: WlBuffer,
    used: bool,
    resizing: bool,
}

impl Buffer {
    pub fn new(queue: QueueHandle<Dispatcher>, shm: &wl_shm::WlShm, id: u32) -> Result<Self, Error>
    {
        // Integers are normally not represented as text using NUL.
        let name = match CString::new(format!("/ufkbd_pid{}_{}", process::id(), id)) {
            Ok(name) => name,
            Err(e) => return Err(Error::new(ErrorKind::InvalidInput, e)),
        };

        let file = unsafe {
            let fd = libc::shm_open(name.as_ptr(),
                                    libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                                    libc::S_IRUSR | libc::S_IWUSR);
            if fd == -1 {
                return Err(Error::last_os_error());
            }

            File::from_raw_fd(fd)
        };

        /*
         * A buffer must be at least 1x1, and 32 bits per pixel
         * (ARGB8888 and XRGB8888) is supported on all Wayland compositors.
         */
        file.set_len(4)?;

        let map = unsafe { MmapMut::map_mut(&file) }?;

        let pool = shm.create_pool(file.as_fd(), 4, &queue, id);
        let buf = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Xrgb8888,
                                     &queue, id);

        /*
         * This can happen before expanding the file. The shm_unlink(3p) manual
         * says:
         *
         *     If one or more references to the shared memory object exist when
         *     the object is unlinked, the name shall be removed before
         *     shm_unlink() returns, but the removal of the memory object
         *     contents shall be postponed until all open and map references to
         *     the shared memory object have been removed.
         */
        unsafe {
            libc::shm_unlink(name.as_ptr());
        };

        Ok(Self {
            file, map,
            id, capacity: 4, width: 1, height: 1,
            queue, pool, buf,
            used: false, resizing: false,
        })
    }

    /*
     * Commit a pending resize operation on buffer. The caller is responsible
     * for checking if the buffer is used, allowing the check to be skipped when
     * releasing the buffer.
     */
    fn commit_resize(&mut self)
    {
        if self.resizing {
            self.buf = self.pool.create_buffer(0, self.width, self.height,
                                               self.width * 4,
                                               wl_shm::Format::Xrgb8888,
                                               &self.queue, self.id);
            self.resizing = false;
        }
    }

    /*
     * Begin a resize operation. The shared memory object gets expanded if the
     * new size can't fit. The operation will be completed when the buffer is
     * not used.
     */
    pub fn resize(&mut self, width: i32, height: i32) -> Result<(), Error>
    {
        if width <= 0 || height <= 0 {
            return Err(Error::new(ErrorKind::InvalidInput, "Buffer must be at least 1x1"));
        }

        // Expand as necessary. Shrinking is not supported by wl_shm_pool.resize().
        if self.capacity < width * height * 4 {
            self.capacity = width * height * 4;

            let filelen = self.capacity as u64;
            self.file.set_len(filelen)?;

            self.pool.resize(self.capacity);

            self.map = unsafe { MmapMut::map_mut(&self.file) }?;
        }

        // Try to resize the buffer now, otherwise defer it.
        self.width = width;
        self.height = height;
        self.resizing = true;
        if !self.used {
            self.commit_resize();
        }

        Ok(())
    }

    /*
     * Get the contents of the buffer. After rendering, the buffer is the
     * program's only copy of the rendered image. The contents of this buffer
     * can be written to the unused buffer in double-buffering.
     */
    pub fn image<'a>(&'a self) -> ImgRef<'a, BGRA<u8>>
    {
        let ptr = self.map.as_ref().as_bgra();
        ImgRef::new(ptr, self.width as usize, self.height as usize)
    }

    /*
     * Consume a buffer to be drawn to and optionally used by a surface. If the
     * OSK is hidden, do not attach the surface.
     */
    pub fn consume<'a>(&'a mut self, surface: Option<&WlSurface>) -> Option<ImgRefMut<'a, BGRA<u8>>>
    {
        if self.used {
            return None;
        }

        self.used = true;

        if let Some(surface) = surface {
            surface.attach(Some(&self.buf), 0, 0);
        }

        // The ARGB format is in little endian.
        let ptr = self.map.as_mut().as_bgra_mut();
        let img = ImgRefMut::new(ptr, self.width as usize, self.height as usize);

        Some(img)
    }

    /*
     * Allow the buffer to be drawn to again and commit any pending resizes.
     * When a double-buffered surface is resized, the used and unused buffer
     * both need to resize eventually.
     */
    pub fn release(&mut self)
    {
        self.used = false;
        self.commit_resize();
    }

    /*
     * Write an image to the buffer for the next consumption. This is used in
     * double-buffering to ensure both buffers have up-to-date contents without
     * redrawing every time.
     */
    pub fn writeback(&mut self, src: &ImgRef<BGRA<u8>>, x: usize, y: usize)
    {
        if self.used {
            panic!("Writeback must always be on an inactive buffer");
        }

        // The ARGB format is in little endian.
        let ptr = self.map.as_mut().as_bgra_mut();
        let mut img = ImgRefMut::new(ptr, self.width as usize, self.height as usize);
        let mut subimg = img.sub_image_mut(x, y, src.width(), src.height());

        for (dest, src) in iter::zip(subimg.pixels_mut(), src.pixels()) {
            *dest = src;
        }
    }

    #[inline(always)]
    pub fn size(&self) -> (i32, i32)
    {
        (self.width, self.height)
    }
}
