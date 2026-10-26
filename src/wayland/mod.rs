// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

mod buffer;
mod dispatcher;
mod keyboard;
mod protocols;
mod seat;
mod surface;

pub use self::buffer::Buffer;
pub use self::dispatcher::Dispatcher;
pub use self::dispatcher::VisibilityToggle;
pub use self::keyboard::VirtualKeyboard;
pub use self::seat::Seat;
pub use self::surface::Surface;

pub use wayland_protocols_wlr::layer_shell::v1::client as wlr_layer_shell_unstable_v1;
pub use wayland_protocols::wp::fractional_scale::v1::client as fractional_scale_v1;
pub use wayland_protocols::wp::input_method::zv1::client as input_method_unstable_v1;
pub use wayland_protocols::wp::text_input::zv3::client as text_input_unstable_v3;
pub use wayland_protocols::wp::viewporter::client as viewporter;

pub use self::protocols::input_method_unstable_v2;
pub use self::protocols::virtual_keyboard_unstable_v1;
