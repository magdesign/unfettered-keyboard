// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

use crate::core::Button;
use crate::core::Configuration;
use crate::core::Display;
use crate::core::Graphics;
use crate::core::Haptics;
use crate::core::Layout;
use crate::core::LayoutError;
use crate::wayland::Seat;
use crate::wayland::Surface;
use crate::wayland::VirtualKeyboard;
use crate::wayland::wlr_layer_shell_unstable_v1::zwlr_layer_shell_v1;
use crate::wayland::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1;
use crate::wayland::fractional_scale_v1::wp_fractional_scale_manager_v1;
use crate::wayland::fractional_scale_v1::wp_fractional_scale_v1;
use crate::wayland::input_method_unstable_v1::zwp_input_method_context_v1;
use crate::wayland::input_method_unstable_v1::zwp_input_method_v1;
use crate::wayland::input_method_unstable_v2::zwp_input_method_manager_v2;
use crate::wayland::input_method_unstable_v2::zwp_input_method_v2;
use crate::wayland::viewporter::wp_viewporter;
use crate::wayland::viewporter::wp_viewport;
use crate::wayland::virtual_keyboard_unstable_v1::zwp_virtual_keyboard_manager_v1;
use crate::wayland::virtual_keyboard_unstable_v1::zwp_virtual_keyboard_v1;
use reis::PendingRequestResult;
use reis::ei::Context;
use reis::ei::Event;
use reis::ei::connection;
use reis::ei::device;
use reis::ei::keyboard;
use reis::ei::seat;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::io::Error;
use std::sync::Arc;
use std::sync::Mutex;
use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::Proxy;
use wayland_client::QueueHandle;
use wayland_client::WEnum;
use wayland_client::globals::BindError;
use wayland_client::globals::GlobalList;
use wayland_client::globals::GlobalListContents;
use wayland_client::protocol::wl_buffer;
use wayland_client::protocol::wl_compositor;
use wayland_client::protocol::wl_pointer;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat;
use wayland_client::protocol::wl_shm;
use wayland_client::protocol::wl_shm_pool;
use wayland_client::protocol::wl_surface;
use wayland_client::protocol::wl_touch;

pub enum ApplicationError {
    Layout(LayoutError),
    Global(String, BindError),
    Surface(Error),
}

impl Debug for ApplicationError {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> Result<(), std::fmt::Error>
    {
        match self {
            Self::Layout(e) => {
                write!(fmt, "loading keyboard layouts: {:?}", e)?;
            },
            Self::Global(n, e) => {
                write!(fmt, "getting required Wayland {} API: {:?}", n, e)?;
            },
            Self::Surface(e) => {
                write!(fmt, "creating display window: {:?}", e)?;
            },
        }
        Ok(())
    }
}

pub trait VisibilityToggle: Send {
    fn set_visible(&self, visible: bool);
}

pub struct Dispatcher {
    seat: Seat<Surface, VirtualKeyboard>,
    gfx: Arc<Mutex<Graphics<Surface>>>,
    osk: Option<Box<dyn VisibilityToggle>>,

    libei_caps: u64,
}

impl Dispatcher {
    pub fn new(cfg: &Configuration, queue: QueueHandle<Self>, globals: &GlobalList)
            -> Result<Self, ApplicationError>
    {
        let shm = globals.bind(&queue, 1..=1, ())
                         .map_err(|e| ApplicationError::Global(String::from("wl_shm"), e))?;
        let compositor = globals.bind(&queue, 4..=6, ())
                         .map_err(|e| ApplicationError::Global(String::from("wl_compositor"), e))?;
        let seat = globals.bind(&queue, 5..=9, ())
                         .map_err(|e| ApplicationError::Global(String::from("wl_seat"), e))?;
        let layer_shell = globals.bind(&queue, 3..=4, ())
                         .map_err(|e| ApplicationError::Global(String::from("zwlr_layer_shell_v1"), e))?;

        let vk_man = match globals.bind(&queue, 1..=1, ()) {
            Ok(g) => Some(g),
            Err(_) => None,
        };

        let im_man = if cfg.wayland_im_enable() {
            let _: Result<zwp_input_method_v1::ZwpInputMethodV1, BindError>
                  = globals.bind(&queue, 1..=1, ());

            globals.bind(&queue, 1..=1, ()).ok()
        } else {
            None
        };

        let frac_scale_man = globals.bind(&queue, 1..=1, ()).ok();
        let vper = globals.bind(&queue, 1..=1, ()).ok();

        let disp = Surface::new(cfg, queue.clone(),
                                &shm, compositor, layer_shell,
                                frac_scale_man, vper)
            .map_err(|e| ApplicationError::Surface(e))?;
        let gfx = Graphics::new(disp, cfg);
        let gfx = Mutex::new(gfx);
        let gfx = Arc::new(gfx);

        let vk = VirtualKeyboard::new(&queue, &vk_man, &im_man, &seat);

        let layout = Layout::load(cfg).map_err(|e| ApplicationError::Layout(e))?;
        let seat = Seat::new(cfg, layout, vk, gfx.clone(), queue.clone(), seat);

        Ok(Dispatcher {
            seat,
            gfx,
            osk: None,

            libei_caps: 0,
        })
    }

    #[inline(always)]
    pub fn button(&self) -> &Button<Surface, VirtualKeyboard>
    {
        self.seat.button()
    }

    #[allow(dead_code)]
    #[inline(always)]
    pub fn graphics(&self) -> Arc<Mutex<Graphics<Surface>>>
    {
        self.gfx.clone()
    }

    #[allow(dead_code)]
    pub fn set_osk(&mut self, osk: Box<dyn VisibilityToggle>)
    {
        self.osk = Some(osk);
    }

    #[allow(dead_code)]
    pub fn set_haptics(&mut self, haptics: Box<dyn Haptics>)
    {
        self.seat.button_mut().set_haptics(haptics);
    }

    pub fn dispatch_timers(&mut self)
    {
        self.seat.button_mut().dispatch_timers();
    }

    fn show(&self)
    {
        match &self.osk {
            Some(osk) => osk.set_visible(true),
            None => {
                let mut gfx = self.gfx.lock().unwrap();
                gfx.display_mut().show();
            },
        }
    }

    fn hide(&self)
    {
        match &self.osk {
            Some(osk) => osk.set_visible(false),
            None => {
                let mut gfx = self.gfx.lock().unwrap();
                gfx.display_mut().hide();
            },
        }
    }
}

#[allow(dead_code)]
impl Dispatcher {
    fn dispatch_ei_connection(&mut self,
                              _conn: connection::Connection,
                              evt: connection::Event)
    {
        match evt {
            connection::Event::Ping { ping } => {
                ping.done(0);
            },
            connection::Event::Seat { .. } => (),
            _ => eprintln!("warn: unknown ei_connection event emitted"),
        };
    }

    fn dispatch_ei_device(&mut self,
                          dev: device::Device,
                          evt: device::Event)
    {
        match evt {
            device::Event::DeviceType { .. } => (),
            device::Event::Done => (),
            device::Event::Frame { serial, .. } => {
                self.seat.keyboard_mut().set_ei_serial(serial);
            },
            device::Event::Interface { object } => {
                if let Some(kbd) = object.downcast() {
                    self.seat.keyboard_mut().set_ei_keyboard(dev, kbd);
                    self.seat.button_mut().update_keys_supported();
                }
            },
            device::Event::Name { .. } => (),
            device::Event::Resumed { serial } => {
                self.seat.keyboard_mut().set_ei_serial(serial);
                dev.start_emulating(serial, 0);
            },
            _ => eprintln!("warn: unknown ei_device event emitted"),
        };
    }

    fn dispatch_ei_keyboard(&mut self, _kbd: keyboard::Keyboard, evt: keyboard::Event)
    {
        match evt {
            _ => eprintln!("warn: unknown ei_keyboard event emitted"),
        };
    }

    fn dispatch_ei_seat(&mut self, seat: seat::Seat, evt: seat::Event)
    {
        match evt {
            seat::Event::Capability { mask, interface } => {
                if interface == "ei_keyboard" {
                    self.libei_caps |= mask;
                }
            },
            seat::Event::Done => {
                seat.bind(self.libei_caps);
            },
            seat::Event::Device { .. } => (),
            seat::Event::Name { .. } => (),
            _ => eprintln!("warn: unknown ei_seat event emitted"),
        };
    }

    pub fn dispatch_ei(&mut self, ctx: &Context)
    {
        while let Some(evt) = ctx.pending_event() {
            let evt = match evt {
                PendingRequestResult::Request(e) => e,
                _ => panic!(),
            };

            match evt {
                Event::Connection(c, e) => self.dispatch_ei_connection(c, e),
                Event::Device(d, e) => self.dispatch_ei_device(d, e),
                Event::Keyboard(k, e) => self.dispatch_ei_keyboard(k, e),
                Event::Seat(s, e) => self.dispatch_ei_seat(s, e),
                _ => eprintln!("warn: unknown libei event emitted"),
            };
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, u32> for Dispatcher {
    fn event(ctx: &mut Dispatcher,
             _buf: &wl_buffer::WlBuffer,
             evt: <wl_buffer::WlBuffer as Proxy>::Event,
             data: &u32,
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        match evt {
            wl_buffer::Event::Release => {
                let mut gfx = ctx.gfx.lock().unwrap();
                gfx.display_mut().release(*data);
            },
            _ => eprintln!("warn: unknown wl_buffer event emitted"),
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for Dispatcher {
    fn event(_ctx: &mut Dispatcher,
             _comp: &wl_compositor::WlCompositor,
             _evt: <wl_compositor::WlCompositor as Proxy>::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        eprintln!("warn: unknown wl_compositor event emitted");
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Dispatcher {
    fn event(ctx: &mut Dispatcher,
             _ptr: &wl_pointer::WlPointer,
             evt: <wl_pointer::WlPointer as Proxy>::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        match evt {
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                ctx.seat.ptr_motion(surface_x, surface_y);
            },
            wl_pointer::Event::Button { state: WEnum::Value(state), .. } => {
                ctx.seat.ptr_button(state);
            },
            wl_pointer::Event::Frame => {
                ctx.seat.commit();
            },
            wl_pointer::Event::Enter { .. } => (),
            wl_pointer::Event::Leave { .. } => (),
            _ => eprintln!("warn: unknown wl_pointer event emitted"),
        };
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Dispatcher {
    fn event(_ctx: &mut Dispatcher,
             _reg: &wl_registry::WlRegistry,
             _evt: <wl_registry::WlRegistry as Proxy>::Event,
             _data: &GlobalListContents,
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Dispatcher {
    fn event(ctx: &mut Dispatcher,
             _seat: &wl_seat::WlSeat,
             evt: <wl_seat::WlSeat as Proxy>::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        match evt {
            wl_seat::Event::Name { .. } => (),
            wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } => {
                ctx.seat.set_capabilities(caps);
            },
            _ => eprintln!("warn: unknown wl_seat event emitted"),
        };
    }
}

impl Dispatch<wl_shm::WlShm, ()> for Dispatcher {
    fn event(_ctx: &mut Dispatcher,
             _shm: &wl_shm::WlShm,
             evt: <wl_shm::WlShm as Proxy>::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        match evt {
            wl_shm::Event::Format { .. } => (),
            _ => eprintln!("warn: unknown wl_shm event emitted"),
        };
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, u32> for Dispatcher {
    fn event(_ctx: &mut Dispatcher,
             _pool: &wl_shm_pool::WlShmPool,
             _evt: <wl_shm_pool::WlShmPool as Proxy>::Event,
             _data: &u32,
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        eprintln!("warn: unknown wl_shm_pool event emitted");
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for Dispatcher {
    fn event(_ctx: &mut Dispatcher,
             _pool: &wl_surface::WlSurface,
             evt: <wl_surface::WlSurface as Proxy>::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        match evt {
            wl_surface::Event::PreferredBufferTransform { .. } => (),
            wl_surface::Event::PreferredBufferScale { .. } => (), // TODO
            _ => eprintln!("warn: unknown wl_surface event emitted"),
        }
    }
}

impl Dispatch<wl_touch::WlTouch, ()> for Dispatcher {
    fn event(ctx: &mut Dispatcher,
             _ptr: &wl_touch::WlTouch,
             evt: <wl_touch::WlTouch as Proxy>::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        match evt {
            wl_touch::Event::Down { id, x, y, .. } => {
                ctx.seat.touch_press(id, x, y);
            },
            wl_touch::Event::Motion { id, x, y, .. } => {
                ctx.seat.touch_pos(id, x, y);
            },
            wl_touch::Event::Up { id, .. } => {
                ctx.seat.touch_release(id);
            },
            wl_touch::Event::Frame => {
                ctx.seat.commit();
            },
            _ => eprintln!("warn: unknown wl_touch event emitted"),
        };
    }
}

impl Dispatch<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, ()> for Dispatcher {
    fn event(_ctx: &mut Dispatcher,
             _pool: &wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
             _evt: wp_fractional_scale_manager_v1::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        eprintln!("warn: unknown wp_fractional_scale_manager_v1 event emitted");
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, ()> for Dispatcher {
    fn event(ctx: &mut Dispatcher,
             _pool: &wp_fractional_scale_v1::WpFractionalScaleV1,
             evt: wp_fractional_scale_v1::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        match evt {
            wp_fractional_scale_v1::Event::PreferredScale { scale } => {
                let mut gfx = ctx.gfx.lock().unwrap();

                if scale == gfx.display().scale() {
                    return;
                }

                let old_scale = gfx.display().scale();
                let (width, height) = gfx.display().size();
                let (width, height) = (width * scale / old_scale,
                                       height * scale / old_scale);

                gfx.display_mut().update_scale(scale);

                if gfx.display().configured() {
                    let layout = ctx.seat.layout();
                    let mod_state = ctx.seat.mod_state();

                    gfx.resize(layout, mod_state, width, height);

                    let (width, height) = gfx.display().size();
                    let width = width as f64 * 120.0 / scale as f64;
                    let height = height as f64 * 120.0 / scale as f64;

                    ctx.seat.set_scale(layout.width() / width,
                                       layout.height() / height);
                }
            },
            _ => eprintln!("warn: unknown wp_fractional_scale_v1 event emitted"),
        };
    }
}

impl Dispatch<wp_viewporter::WpViewporter, ()> for Dispatcher {
    fn event(_ctx: &mut Dispatcher,
             _pool: &wp_viewporter::WpViewporter,
             _evt: wp_viewporter::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        eprintln!("warn: unknown wp_viewporter event emitted");
    }
}

impl Dispatch<wp_viewport::WpViewport, ()> for Dispatcher {
    fn event(_ctx: &mut Dispatcher,
             _pool: &wp_viewport::WpViewport,
             _evt: wp_viewport::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        eprintln!("warn: unknown wp_viewport event emitted");
    }
}

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for Dispatcher {
    fn event(_ctx: &mut Dispatcher,
             _pool: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
             _evt: <zwlr_layer_shell_v1::ZwlrLayerShellV1 as Proxy>::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        eprintln!("warn: unknown zwlr_layer_shell_v1 event emitted");
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for Dispatcher {
    fn event(ctx: &mut Dispatcher,
             _surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
             evt: zwlr_layer_surface_v1::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        match evt {
            zwlr_layer_surface_v1::Event::Configure { width, height, serial } => {
                let mut gfx = ctx.gfx.lock().unwrap();

                gfx.display_mut().ack_configure(serial);

                let layout = ctx.seat.layout();
                let mod_state = ctx.seat.mod_state();

                let scale = gfx.display().scale();
                let (width, height) = (width * scale / 120, height * scale / 120);
                gfx.resize(layout, mod_state, width, height);

                let disp = gfx.display();
                let scale = disp.scale() as f64;
                let (width, height) = disp.size();
                let width = width as f64 * 120.0 / scale;
                let height = height as f64 * 120.0 / scale;

                ctx.seat.set_scale(layout.width() / width,
                                   layout.height() / height);
            },
            _ => eprintln!("warn: unknown zwlr_layer_surface_v1 event emitted"),
        }
    }
}

impl Dispatch<zwp_input_method_context_v1::ZwpInputMethodContextV1, ()> for Dispatcher {
    fn event(ctx: &mut Dispatcher,
             _imc: &zwp_input_method_context_v1::ZwpInputMethodContextV1,
             evt: zwp_input_method_context_v1::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        match evt {
            zwp_input_method_context_v1::Event::SurroundingText { .. } => (),
            zwp_input_method_context_v1::Event::Reset => (),
            zwp_input_method_context_v1::Event::ContentType { .. } => (),
            zwp_input_method_context_v1::Event::CommitState { serial } => {
                ctx.seat.keyboard_mut().set_im_serial(serial);
            },
            zwp_input_method_context_v1::Event::PreferredLanguage { .. } => (),
            _ => eprintln!("warn: unknown zwp_input_method_context_v1 event emitted"),
        }
    }
}

impl Dispatch<zwp_input_method_manager_v2::ZwpInputMethodManagerV2, ()> for Dispatcher {
    fn event(_ctx: &mut Dispatcher,
             _im_man: &zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
             _evt: zwp_input_method_manager_v2::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        eprintln!("warn: unknown zwp_input_method_manager_v2 event emitted");
    }
}

impl Dispatch<zwp_input_method_v1::ZwpInputMethodV1, ()> for Dispatcher {
    fn event(ctx: &mut Dispatcher,
             _im: &zwp_input_method_v1::ZwpInputMethodV1,
             evt: zwp_input_method_v1::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        match evt {
            zwp_input_method_v1::Event::Activate { id } => {
                ctx.seat.set_text_supported(true);
                ctx.seat.keyboard_mut().set_input_method_context(Some(id));
                ctx.show();
            },
            zwp_input_method_v1::Event::Deactivate { .. } => {
                ctx.seat.set_text_supported(false);
                ctx.seat.keyboard_mut().set_input_method_context(None);
                ctx.hide();
            },
            _ => eprintln!("warn: unknown zwp_input_method_v1 event emitted"),
        }
    }

    wayland_client::event_created_child!(Dispatcher, zwp_input_method_v1::ZwpInputMethodV1, [
        zwp_input_method_v1::EVT_ACTIVATE_OPCODE
            => (zwp_input_method_context_v1::ZwpInputMethodContextV1, ()),
    ]);
}

impl Dispatch<zwp_input_method_v2::ZwpInputMethodV2, ()> for Dispatcher {
    fn event(ctx: &mut Dispatcher,
             _im: &zwp_input_method_v2::ZwpInputMethodV2,
             evt: zwp_input_method_v2::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        match evt {
            zwp_input_method_v2::Event::Activate => {
                ctx.seat.set_text_supported(true);
                ctx.show();
            },
            zwp_input_method_v2::Event::Deactivate => {
                ctx.seat.set_text_supported(false);
                ctx.hide();
            },
            zwp_input_method_v2::Event::TextChangeCause { .. } => (),
            zwp_input_method_v2::Event::ContentType { .. } => (),
            zwp_input_method_v2::Event::SurroundingText { .. } => (),
            zwp_input_method_v2::Event::Done => {
                ctx.seat.keyboard_mut().done();
            },
            _ => eprintln!("warn: unknown zwp_input_method_v2 event emitted"),
        }
    }
}

impl Dispatch<zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, ()> for Dispatcher {
    fn event(_ctx: &mut Dispatcher,
             _vk_man: &zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
             _evt: zwp_virtual_keyboard_manager_v1::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        eprintln!("warn: unknown zwp_virtual_keyboard_manager_v1 event emitted");
    }
}

impl Dispatch<zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1, ()> for Dispatcher {
    fn event(_ctx: &mut Dispatcher,
             _vk: &zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
             _evt: zwp_virtual_keyboard_v1::Event,
             _data: &(),
             _conn: &Connection,
             _qh: &QueueHandle<Dispatcher>)
    {
        eprintln!("warn: unknown zwp_virtual_keyboard_v1 event emitted");
    }
}
