// SPDX-License-Identifier: GPL-3.0-only
/*
 * Copyright (c) 2024, Unfettered Keyboard Contributors
 */

use crate::core::Configuration;
use crate::core::KeyboardColors;
use crate::core::Layout;
use crate::core::button::ModState;
use crate::core::layout::Key;
use crate::core::layout::Part;
use fontconfig::Fontconfig;
use fontconfig::Pattern;
use fontconfig::UnicodeCoverage;
use freetype::freetype;
use imgref::ImgRef;
use imgref::ImgRefMut;
use imgref::ImgVec;
use rgb::alt::BGR;
use rgb::alt::BGRA;
use rgb::ComponentMap;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::cmp;
use std::iter;
use std::ptr;
use std::sync::atomic::AtomicPtr;

const KEYCAP_RADIUS: usize = 12;

fn convert_gray_to_bgrx(mut dest: ImgRefMut<BGRA<u8>>, src: ImgRef<u8>, fg_color: BGR<f32>)
{
    for (dest, src) in iter::zip(dest.rows_mut(), src.rows()) {
        for (dest, src) in iter::zip(dest, src) {
            dest.r = dest.r.saturating_add((*src as f32 * fg_color.r) as u8);
            dest.g = dest.g.saturating_add((*src as f32 * fg_color.g) as u8);
            dest.b = dest.b.saturating_add((*src as f32 * fg_color.b) as u8);
        }
    }
}

fn copy_image(mut dest: ImgRefMut<u8>, src: ImgRef<u8>)
{
    for (dest, src) in iter::zip(dest.pixels_mut(), src.pixels()) {
        *dest = src.saturating_add(*dest);
    }
}

fn fill_image(dest: &mut ImgRefMut<BGRA<u8>>, src: BGRA<u8>)
{
    for dest in dest.pixels_mut() {
        *dest = src;
    }
}

fn fill_rounded_rect(dest: &mut ImgRefMut<BGRA<u8>>, src: BGRA<u8>, radius: usize)
{
    if radius == 0 {
        fill_image(dest, src);
        return;
    }

    let width = dest.width();
    let height = dest.height();
    let radius = radius.min(width / 2).min(height / 2);

    for y in 0..height {
        for x in 0..width {
            let in_corner = if x < radius && y < radius {
                (x as isize - radius as isize).pow(2) + (y as isize - radius as isize).pow(2)
                    <= (radius as isize).pow(2)
            } else if x >= width - radius && y < radius {
                (x as isize - (width - radius) as isize).pow(2)
                    + (y as isize - radius as isize).pow(2)
                    <= (radius as isize).pow(2)
            } else if x < radius && y >= height - radius {
                (x as isize - radius as isize).pow(2)
                    + (y as isize - (height - radius) as isize).pow(2)
                    <= (radius as isize).pow(2)
            } else if x >= width - radius && y >= height - radius {
                (x as isize - (width - radius) as isize).pow(2)
                    + (y as isize - (height - radius) as isize).pow(2)
                    <= (radius as isize).pow(2)
            } else {
                true
            };

            if in_corner {
                dest[(x, y)] = src;
            }
        }
    }
}

fn bitmap_to_imgref<'a>(bitmap: &'a freetype::FT_Bitmap) -> ImgRef<'a, u8>
{
    unsafe {
        let buf = &*ptr::slice_from_raw_parts(bitmap.buffer,
                                              bitmap.pitch as usize
                                            * bitmap.rows as usize);
        ImgRef::new_stride(buf,
                           bitmap.width as usize,
                           bitmap.rows as usize,
                           bitmap.pitch as usize)
    }
}

#[derive(Clone, Copy)]
enum Anchor {
    Min,
    Center,
    Max,
}

pub trait Display {
    fn size(&self) -> (u32, u32);

    fn begin<'a>(&'a mut self) -> ImgRefMut<'a, BGRA<u8>>;
    fn resize(&mut self, width: u32, height: u32);
    fn end(&mut self, x: u32, y: u32, width: u32, height: u32);
}

struct TextRaster {
    img: ImgVec<u8>,
    y: usize,
}

pub struct Graphics<D: Display> {
    disp: D,
    fonts: Vec<(String, freetype::FT_Long)>,
    fontbufs: Vec<Vec<u8>>,
    labels: HashMap<String, TextRaster>,
    sublabels: HashMap<String, TextRaster>,
    ft: AtomicPtr<freetype::FT_LibraryRec_>,

    x_scale: f64,
    y_scale: f64,

    colors: KeyboardColors,
    keycap_padding: f64,
}

struct Label<'a> {
    label: &'a str,
    secondary: bool,
    small: bool,
}

const LABELS: [Label; 25] = [
    Label { label: "123+",
            secondary: true, small: true, },
    Label { label: "ABC",
            secondary: true, small: true, },
    Label { label: "Alt",
            secondary: true, small: true, },
    Label { label: "Ctrl",
            secondary: true, small: true, },
    Label { label: "Esc",
            secondary: true, small: true, },
    Label { label: "F11",
            secondary: false, small: true, },
    Label { label: "F12",
            secondary: false, small: true, },
    Label { label: "Fn",
            secondary: true, small: true, },
    Label { label: "Ins",
            secondary: true, small: true, },
    Label { label: "Meta",
            secondary: true, small: true, },
    Label { label: "πλ∇¬",
            secondary: true, small: true, },
    Label { label: "\u{2190}", // LEFTWARDS ARROW
            secondary: true, small: false, },
    Label { label: "\u{2191}", // UPWARDS ARROW
            secondary: true, small: false, },
    Label { label: "\u{2192}", // RIGHTWARDS ARROW
            secondary: true, small: false, },
    Label { label: "\u{2193}", // DOWNWARDS ARROW
            secondary: true, small: false, },
    Label { label: "\u{21D1}", // UPWARDS DOUBLE ARROW
            secondary: true, small: false, },
    Label { label: "\u{21D3}", // DOWNWARDS DOUBLE ARROW
            secondary: true, small: false, },
    Label { label: "\u{21E4}", // LEFTWARDS ARROW TO BAR
            secondary: true, small: false, },
    Label { label: "\u{21E5}", // RIGHTWARDS ARROW TO BAR
            secondary: true, small: false, },
    Label { label: "\u{21E7}", // UPWARDS WHITE ARROW
            secondary: true, small: false, },
    Label { label: "\u{2326}", // ERASE TO THE RIGHT
            secondary: true, small: false, },
    Label { label: "\u{232B}", // ERASE TO THE LEFT
            secondary: true, small: false, },
    Label { label: "\u{23CE}", // RETURN SYMBOL
            secondary: true, small: false, },
    Label { label: "\u{2423}", // OPEN BOX
            secondary: true, small: true, },
    Label { label: "\u{2B7E}", // HORIZONTAL TAB KEY
            secondary: true, small: true, },
];

const ANCHORS: [(Anchor, Anchor); 8] = [
    (Anchor::Min, Anchor::Min),
    (Anchor::Max, Anchor::Min),
    (Anchor::Min, Anchor::Max),
    (Anchor::Max, Anchor::Max),
    (Anchor::Min, Anchor::Center),
    (Anchor::Max, Anchor::Center),
    (Anchor::Center, Anchor::Min),
    (Anchor::Center, Anchor::Max),
];

impl<D: Display> Graphics<D> {
    pub fn new(disp: D, cfg: &Configuration) -> Graphics<D>
    {
        let ft = unsafe {
            let mut ft = std::ptr::null_mut();
            let err = freetype::FT_Init_FreeType(&mut ft);
            if err != 0 {
                panic!("FreeType error {}", err);
            }

            AtomicPtr::new(ft)
        };

        /*
         * We want a pattern with default and config substitution, so use the
         * side effect in font_match(). The first font might not provide all
         * glyphs needed, so the returned font is unused.
         */
        let fc = Fontconfig::new().unwrap();
        let mut pattern = Pattern::new(&fc).unwrap();
        pattern.font_match().unwrap();

        let fonts = pattern.sort_fonts(UnicodeCoverage::Trim).unwrap();

        let mut fontfiles = Vec::new();

        for font in fonts.iter() {
            let fname = match font.filename() {
                Ok(fname) => String::from(fname),
                Err(_) => continue,
            };

            let idx = match font.face_index() {
                Ok(idx) => idx.into(),
                Err(_) => 0,
            };

            fontfiles.push((fname, idx));
        }

        Graphics {
            disp,
            ft,
            fonts: fontfiles,
            fontbufs: Vec::new(),
            labels: HashMap::new(),
            sublabels: HashMap::new(),

            x_scale: 0.0,
            y_scale: 0.0,

            colors: cfg.colors().clone(),
            keycap_padding: cfg.keycap_padding(),
        }
    }

    fn is_label_small(label: &str) -> bool
    {
        match LABELS.binary_search_by_key(&label, |&Label { label, .. }| label) {
            Ok(idx) => LABELS[idx].small,
            Err(_) => false,
        }
    }

    fn open_font(&mut self, idx: usize) -> Option<freetype::FT_Face>
    {
        // Read the font if it is not already cached.
        if self.fontbufs.len() <= idx {
            let mut f = File::open(&self.fonts[idx].0).ok()?;

            let mut fontbuf = Vec::new();
            f.read_to_end(&mut fontbuf).ok()?;

            self.fontbufs.push(fontbuf);
        }

        let fontbuf = &self.fontbufs[idx];

        unsafe {
            let mut ftface = std::ptr::null_mut();
            freetype::FT_New_Memory_Face(*self.ft.get_mut(),
                                         fontbuf.as_ptr(),
                                         fontbuf.len() as freetype::FT_Long,
                                         self.fonts[idx].1 as freetype::FT_Long,
                                         &mut ftface);
            if ptr::eq(ftface, ptr::null_mut()) {
                None
            } else {
                Some(ftface)
            }
        }
    }

    fn can_font_render_text(face: Option<freetype::FT_Face>, text: &str) -> bool
    {
        let face = match face {
            Some(face) => face,
            None => return false,
        };

        for c in text.chars() {
            unsafe {
                if freetype::FT_Get_Char_Index(face, c.into()) == 0 {
                    return false;
                }
            }
        }

        true
    }

    fn open_font_for_text(&mut self, text: &str) -> Option<freetype::FT_Face>
    {
        let mut ftface = self.open_font(0);
        let mut i = 1;

        while !Self::can_font_render_text(ftface, text) {
            if let Some(ftface) = ftface {
                unsafe {
                    freetype::FT_Done_Face(ftface);
                }
            }

            ftface = self.open_font(i);

            i += 1;
            if i >= self.fonts.len() {
                // Fall back to the first font with placeholder glyphs
                return self.open_font(0);
            }
        }

        ftface
    }

    fn rasterize_text(&mut self, size: f64, text: &str) -> TextRaster
    {
        let ftface = self.open_font_for_text(text).unwrap();
        unsafe {
            freetype::FT_Set_Char_Size(ftface, (size * 64.0) as i64, 0, 0, 0);
        };

        let (metrics, units_per_em, bbox) = unsafe {
            ((*(*ftface).size).metrics,
            (*ftface).units_per_EM,
            (*ftface).bbox)
        };

        let mut width = 0;
        let glyph_width = ((bbox.xMax - bbox.xMin) as usize
                        * metrics.x_ppem as usize)
                        .div_ceil(units_per_em as usize);

        let stride = if text.is_empty() {
            1
        } else {
            metrics.max_advance as usize / 64 * (text.len() - 1) + glyph_width
        };

        let y_max = cmp::max(metrics.ascender as isize / 64,
                             bbox.yMax as isize
                           * metrics.y_ppem as isize
                           / units_per_em as isize);
        let y_min = cmp::min(metrics.descender as isize / 64,
                             bbox.yMin as isize
                           * metrics.y_ppem as isize
                           / units_per_em as isize);
        let mut y = (y_max - metrics.ascender as isize / 64) as usize;
        let height = (y_max - y_min) as usize;

        let vec: Vec<u8> = vec![0; stride * height];
        let mut img = ImgVec::new(vec, stride, height);

        let mut pen = 0;
        let mut height = (y_max - metrics.descender as isize / 64) as usize;

        for c in text.chars() {
            let glyph = unsafe {
                freetype::FT_Load_Char(ftface, c.into(), 0);
                freetype::FT_Render_Glyph((*ftface).glyph, freetype::FT_Render_Mode::FT_RENDER_MODE_NORMAL);
                *(*ftface).glyph
            };

            if glyph.bitmap.rows != 0 && glyph.bitmap.width != 0 {
                pen = pen.max(-glyph.bitmap_left as isize);

                let dest_x = (pen + glyph.bitmap_left as isize) as usize;
                let dest_y = (y_max - glyph.bitmap_top as isize) as usize;

                y = y.min(dest_y);
                height = height.max(dest_y + glyph.bitmap.rows as usize);

                let src = bitmap_to_imgref(&glyph.bitmap);
                let dest = img.sub_image_mut(dest_x,
                                              dest_y,
                                              glyph.bitmap.width as usize,
                                              glyph.bitmap.rows as usize);
                copy_image(dest, src);
            }

            let new_width = (pen + glyph.bitmap_left as isize
                                 + glyph.bitmap.width as isize) as usize;
            width = width.max(new_width);

            pen += glyph.advance.x as isize / 64;
        }

        unsafe {
            freetype::FT_Done_Face(ftface);
        };

        let mut vec = img.into_buf();
        vec.truncate(stride * height);
        let img = ImgVec::new_stride(vec, width, height, stride);

        TextRaster {
            img,
            y,
        }
    }

    /*
     * Rasterize a key symbol and store it in the labels hash table.
     *
     * We need two functions, one for labels and one for sublabels, in order to
     * borrow the hash tables mutably. The text rasterizing operation borrows
     * the self as mutable to store fonts for later use. Since the self and hash
     * table cannot be mutable at the same time, this cannot be abstracted to a
     * mutable self and mutable hash table.
     */
    fn rasterize_label(&mut self, size: f64, label: &str)
    {
        if !self.labels.contains_key(label) {
            let size = if Self::is_label_small(label) {
                size * 0.75
            } else {
                size
            };

            let img = self.rasterize_text(size, label);
            self.labels.insert(String::from(label), img);
        }
    }

    /*
     * Rasterize a key symbol and store it in the sublabels hash table.
     *
     * We need two functions, one for labels and one for sublabels, in order to
     * borrow the hash tables mutably. The text rasterizing operation borrows
     * the self as mutable to store fonts for later use. Since the self and hash
     * table cannot be mutable at the same time, this cannot be abstracted to a
     * mutable self and mutable hash table.
     */
    fn rasterize_sublabel(&mut self, size: f64, label: &str)
    {
        if !self.sublabels.contains_key(label) {
            let size = if Self::is_label_small(label) {
                size * 0.75
            } else {
                size
            };

            let img = self.rasterize_text(size, label);
            self.sublabels.insert(String::from(label), img);
        }
    }

    fn rasterize_labels(&mut self, layout: &Layout)
    {
        self.labels = HashMap::new();
        self.sublabels = HashMap::new();

        let label_size = self.y_scale * 0.33;
        let sublabel_size = self.y_scale * 0.22;

        for row in layout.rows() {
            self.labels.reserve(row.len());
            self.sublabels.reserve(row.len() * 8);

            for key in row {
                let label = key.parts[0].display_label();
                self.rasterize_label(label_size, label);

                for part in &key.parts[1..] {
                    let label = part.display_label();
                    self.rasterize_sublabel(sublabel_size, label);
                }
            }
        }
    }

    fn draw_label_part(labels: &HashMap<String, TextRaster>,
                       x_anchor: Anchor, y_anchor: Anchor,
                       dest: &mut ImgRefMut<BGRA<u8>>,
                       label: &str, fg_color: BGR<f32>)
    {
        let src = labels.get(label)
                        .expect("Layout and size should be set before drawing");

        let width = src.img.width();
        let height = src.img.height() - src.y;

        let x = match x_anchor {
            Anchor::Min => 0,
            Anchor::Center => (dest.width() - width) / 2,
            Anchor::Max => dest.width() - width,
        };

        let y = match y_anchor {
            Anchor::Min => 0,
            Anchor::Center => (dest.height() - height) / 2,
            Anchor::Max => dest.height() - height,
        };

        let src = src.img.sub_image(0, src.y, width, height);
        let dest = dest.sub_image_mut(x, y, width, height);

        convert_gray_to_bgrx(dest, src, fg_color);
    }

    fn keypart_to_color(part: &Part, sublabel: bool,
                        mod_state: &[ModState], colors: &KeyboardColors) -> BGR<u8>
    {
        let modifier = part.modifier_id();
        if modifier != 0 {
            let modifier = modifier - 1;
            if  mod_state[modifier] == ModState::Locked
             || mod_state[modifier] == ModState::HeldLocked
             || mod_state[modifier] == ModState::HeldLockedPressed {
                return colors.label_locked;
            } else if mod_state[modifier] != ModState::Released {
                return colors.label_pressed;
            }
        }

        if part.presses() != 0 {
            return colors.label_pressed;
        }

        if sublabel {
            return colors.sublabel;
        }

        let label = part.display_label();
        let res = LABELS.binary_search_by_key(&label, |&Label { label, .. }| label);
        if let Ok(idx) = res {
            if LABELS[idx].secondary {
                return colors.sublabel;
            }
        }

        colors.label
    }

    fn draw_key(labels: &HashMap<String, TextRaster>,
                sublabels: &HashMap<String, TextRaster>,
                x_scale: f64, y_scale: f64,
                img: &mut ImgRefMut<BGRA<u8>>,
                key: &Key, mod_state: &[ModState],
                colors: &KeyboardColors, keycap_padding: f64)
    {
        let keycap_padding = (keycap_padding * y_scale) as usize;
        let x1 = (key.x1 * x_scale) as usize + keycap_padding;
        let y1 = (key.y1 * y_scale) as usize + keycap_padding;
        let x2 = (key.x2 * x_scale) as usize - keycap_padding;
        let y2 = (key.y2 * y_scale) as usize - keycap_padding;

        let mut keyimg = img.sub_image_mut(x1, y1, x2 - x1, y2 - y1);

        let mut pressed = false;
        for part in &key.parts {
            let modifier = part.modifier_id();
            if modifier != 0 && mod_state[modifier - 1] != ModState::Released {
                pressed = true;
                break;
            }

            if part.presses() != 0 {
                pressed = true;
                break;
            }
        }

        if pressed {
            fill_rounded_rect(&mut keyimg, colors.keycap_pressed, KEYCAP_RADIUS);
        } else {
            fill_rounded_rect(&mut keyimg, colors.keycap, KEYCAP_RADIUS);
        }

        let color = Self::keypart_to_color(&key.parts[0], false, mod_state, colors);
        let label = key.parts[0].display_label();
        Self::draw_label_part(labels, Anchor::Center, Anchor::Center,
                              &mut keyimg, label, color.map(|c| c as f32 / 255.));

        for i in 0..8 {
            let color = Self::keypart_to_color(&key.parts[i + 1], true, mod_state, colors);
            let label = key.parts[i + 1].display_label();
            Self::draw_label_part(sublabels, ANCHORS[i].0, ANCHORS[i].1,
                                  &mut keyimg, label, color.map(|c| c as f32 / 255.));
        }
    }

    pub fn draw_single(&mut self, key: &Key, mod_state: &[ModState])
    {
        let mut img = self.disp.begin();

        let x1 = (key.x1 * self.x_scale) as u32;
        let y1 = (key.y1 * self.y_scale) as u32;
        let x2 = (key.x2 * self.x_scale) as u32;
        let y2 = (key.y2 * self.y_scale) as u32;

        Self::draw_key(&self.labels, &self.sublabels,
                       self.x_scale, self.y_scale,
                       &mut img, key, mod_state, &self.colors,
                       self.keycap_padding);

        self.disp.end(x1, y1, x2 - x1, y2 - y1);
    }

    pub fn draw_modifiers(&mut self,
                          layout: &Layout,
                          mod_state: &[ModState],
                          modifiers: &[bool])
    {
        for row in layout.rows() {
            for key in row {
                let draw = key.parts.iter().any(|p| p.modifier_id() != 0
                                                 && modifiers[p.modifier_id() - 1]);
                if draw {
                    self.draw_single(key, mod_state);
                }
            }
        }
    }

    pub fn draw(&mut self, layout: &Layout, mod_state: &[ModState])
    {
        let mut img = self.disp.begin();

        fill_image(&mut img, self.colors.background);

        for row in layout.rows() {
            for key in row {
                Self::draw_key(&self.labels, &self.sublabels,
                               self.x_scale, self.y_scale,
                               &mut img, key, mod_state, &self.colors,
                               self.keycap_padding);
            }
        }

        let (width, height) = (img.width() as u32, img.height() as u32);
        self.disp.end(0, 0, width, height);
    }

    pub fn change_layout(&mut self, layout: &Layout, mod_state: &[ModState])
    {
        let (width, height) = self.disp.size();
        self.x_scale = width as f64 / layout.width();
        self.y_scale = height as f64 / layout.height();

        self.rasterize_labels(layout);
        self.draw(layout, mod_state);
    }

    pub fn resize(&mut self,
                  layout: &Layout, mod_state: &[ModState],
                  width: u32, height: u32)
    {
        self.x_scale = width as f64 / layout.width();
        self.y_scale = height as f64 / layout.height();

        self.rasterize_labels(layout);

        self.disp.resize(width, height);
        self.draw(layout, mod_state);
    }

    #[inline(always)]
    pub fn display(&self) -> &D
    {
        &self.disp
    }

    #[inline(always)]
    pub fn display_mut(&mut self) -> &mut D
    {
        &mut self.disp
    }
}

impl<D: Display> Drop for Graphics<D> {
    fn drop(&mut self)
    {
        unsafe {
            freetype::FT_Done_FreeType(*self.ft.get_mut());
        }
    }
}
