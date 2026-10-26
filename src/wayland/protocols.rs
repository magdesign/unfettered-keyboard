// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

pub mod input_method_unstable_v2 {
    use wayland_client;
    use wayland_client::protocol::*;
    use crate::wayland::text_input_unstable_v3::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;

        wayland_scanner::generate_interfaces!("wayland-protocols/input-method-unstable-v2.xml");
    }

    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("wayland-protocols/input-method-unstable-v2.xml");
}

pub mod virtual_keyboard_unstable_v1 {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;

        wayland_scanner::generate_interfaces!("wayland-protocols/virtual-keyboard-unstable-v1.xml");
    }

    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("wayland-protocols/virtual-keyboard-unstable-v1.xml");
}
