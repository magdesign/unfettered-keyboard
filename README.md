The Unfettered Keyboard is an on-screen keyboard for Linux mobile.

# User Interfaces (UIs)

## Phosh

Phosh has a D-Bus API for showing the keyboard without focusing on a text box.
The keyboard can be shown by making a long press on the bottom of the screen.
The API tells the keyboard to activate while making text-emitting keys fall
back to emitting key presses.

Phoc, the compositor, does not implement all Wayland text input protocols. Most
Qt applications require manually activating the keyboard and can only accept
keys supported by xkbcommon (e.g. no "‹", "›", "₹", "₽", "₴", "₿", or "₱").

### Build target

The `ufkbd-gnome` build target is for Phosh.

### Switching keyboards

The keyboard program is specified in the `Exec=` line of
`/usr/share/applications/sm.puri.OSK0.desktop`. Change it to the absolute path
of your keyboard.

Since Phosh 0.50.0 (see [doc](https://phosh.mobi/posts/phosh-osk-interface/)) install
[unfettered-keyboard-phosh.service](resources/unfettered-keyboard-phosh.service) and reboot:

    sudo install -Dm755 target/release/ufkbd-gnome /usr/bin/ufkbd-gnome
    sudo install -Dm644 resources/unfettered-keyboard-phosh.service \
        /usr/lib/systemd/user/mobi.phosh.OSK.service

## Plasma Mobile

Plasma Mobile has no built-in UI to show or hide the keyboard without a text
field.

Kwin does not implement the Wayland virtual keyboard protocol. Instead, it
implements the Emulated Input protocol over D-Bus. This spawns a permission
dialog on startup.

### Build target

The `ufkbd-kde` build target is for Plasma Mobile.

### Switching keyboards

Install [unfettered-keyboard-kde.desktop](resources/unfettered-keyboard-kde.desktop) and
[unfettered-keyboard.svg](resources/unfettered-keyboard.svg) to register the keyboard with KDE:

    sudo install -Dm755 target/release/ufkbd-kde /usr/bin/ufkbd-kde
    sudo install -Dm644 resources/unfettered-keyboard-kde.desktop \
        /usr/share/applications/unfettered-keyboard.desktop
    sudo install -Dm644 resources/unfettered-keyboard.svg \
        /usr/share/icons/hicolor/scalable/apps/unfettered-keyboard.svg

After that you will be able to select the keyboard in Plasma Settings.

## SXMO (Wayland)

SXMO shows and hides the on-screen keyboard by running and terminating it. The
on-screen keyboard does not handle visibility.

### Build target

The `ufkbd-sxmo` build target is for SXMO.

### Switching keyboards

Set the `$KEYBOARD` environment variable to your desired keyboard.
Install [unfettered-keyboard-sxmo.sh](resources/unfettered-keyboard-sxmo.sh) to set it system-wide:

    sudo install -Dm755 target/release/ufkbd-sxmo /usr/bin/ufkbd-sxmo
    sudo install -Dm644 resources/unfettered-keyboard-sxmo.sh \
        /etc/profile.d/unfettered-keyboard.sh

## Other

Most Wayland compositors have no mechanism to show or hide the keyboard without
a text field.

### Build target

The `ufkbd-wl` build target is for most compatible Wayland compositors.

### Switching keyboards

Configure your desired keyboard to run on startup. This may mean a desktop file
in `~/.config/autostart`.

## TTYescape

In the TTY, on-screen keyboards can emit key presses with uinput. A special
Linux keymap API also allows arbitrary text to be emitted. Backspace might not
work as expected when typing multi-byte UTF-8 characters.

### Build target

The `ufkbd-evfb` build target is for the TTY.

## Full-disk encryption on postmarketOS

For full-disk encryption, the password is expected on stdout. The keyboard
needs to process individual key events by itself. It is similar to TTYescape
where the keyboard is rendered to the framebuffer.

### Build target

The `ufkbd-evfbo` build target is for password entry.

### Switching keyboards

Set the KEYBOARD property in `/etc/conf.d/ttyescape.conf` to your desired
keyboard.

# Installation

This program has the following build dependencies:

- [libexpat](https://libexpat.github.io) (Alpine package: `expat-dev`),
  the fast streaming XML parser written in C99 with >90% test coverage.
- [libclang](https://clang.llvm.org) (Alpine package: `clang-libclang`),
  a compiler front-end for the C family of languages (C, C++, Objective-C, and
  Objective-C++).
- [fontconfig](https://www.freedesktop.org/wiki/Software/fontconfig)
  (Alpine package: `fontconfig-dev`),
  the font configuration and customization library.
- [wayland-scanner](https://wayland.freedesktop.org)
  (Alpine package: `wayland-dev`),
  a code generator for extra XML protocols.

You can install it with:

    cargo install --path . --bin <target>

Layouts are essential and not installed by cargo. 
**Install them manually:**
```
sudo mkdir -p /usr/share/unfettered-keyboard/layouts
```
```
	curl -fLO https://github.com/magdesign/unfettered-keyboard/raw/refs/heads/main/resources/layouts/latn_qwerty_us.xml
	sudo install -m 0644 latn_qwerty_us.xml /usr/share/unfettered-keyboard/layouts/
```
```
	curl -fLO https://github.com/magdesign/unfettered-keyboard/raw/refs/heads/main/resources/layouts/bottom_row.xml
	sudo install -m 0644 bottom_row.xml /usr/share/unfettered-keyboard/layouts/
```
```
	curl -fLO https://github.com/magdesign/unfettered-keyboard/raw/refs/heads/main/resources/layouts/numeric.xml
	sudo install -m 0644 numeric.xml /usr/share/unfettered-keyboard/layouts/
```
```
	curl -fLO https://github.com/magdesign/unfettered-keyboard/raw/refs/heads/main/resources/layouts/greekmath.xml
	sudo install -m 0644 greekmath.xml /usr/share/unfettered-keyboard/layouts/
```
```
	curl -fLO https://github.com/magdesign/unfettered-keyboard/raw/refs/heads/main/resources/layouts/emoji.xml
	sudo install -m 0644 emoji.xml /usr/share/unfettered-keyboard/layouts/
```
# Configuration

This keyboard is configured with [YAML](https://yaml.org). The configuration
file can either be user-specific in `$HOME/.config/unfettered-keyboard.yaml` or
system-wide in `/etc/xdg/unfettered-keyboard.yaml`. System-wide configuration
doesn't apply if there is a user configuration file.

The configuration file does not need to exist, and can be empty or omit any
values. The YAML header is also optional.

Configuration example (with defaults):

    %YAML 1.2
    ---
    layout: latn_qwerty_us.xml

    longpress_ms: 600
    repeat_ms: 25

    haptics: true

    wayland:
      height: 185
      input-method: true

    extra_keys:
      - meta
      - alt
      - esc
      - tab

    colors:
      background: [0, 0, 0]
      keycap: [0, 0, 0]
      keycap_pressed: [51, 51, 51]
      label: [255, 255, 255]
      label_locked: [0, 255, 25]
      label_pressed: [0, 153, 255]
      sublabel: [178, 178, 178]

    keycap_padding: 0.0 # Keycap padding in fraction of a key size

# License

Unfettered Keyboard is free software; you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, version 3 of the License.

Prior to version 2.0.0, Gitlab automatically displayed "3.0 or later", but that
was not the intended license as seen in the source code files. Version 2.0.0
ensures that the wrong license would not be displayed on Gitlab.
