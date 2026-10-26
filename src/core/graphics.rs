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
use std::cmp;
use std::ffi::CString;
use std::iter;
use std::ptr;
use std::sync::atomic::AtomicPtr;

const KEYCAP_RADIUS: usize = 12;
const SUBLABEL_MARGIN: usize = 10;

/*
 * Color emoji are drawn at (label font size * EMOJI_SCALE) pixels tall. The
 * label font size is 0.33 of a key height (0.22 for sublabels), so 1.5 gives
 * an emoji of roughly half a key height. Tune to taste.
 */
const EMOJI_SCALE: f64 = 1.5;

/*
 * FreeType constants, spelled out so we do not depend on how the bindings
 * name them.
 */
const FT_LOAD_RENDER: i32 = 1 << 2;
const FT_LOAD_COLOR: i32 = 1 << 20;
const FT_FACE_FLAG_FIXED_SIZES: i64 = 1 << 1;
const FT_FACE_FLAG_COLOR: i64 = 1 << 14;
const FT_PIXEL_MODE_BGRA: u8 = 7;

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

/*
 * Source-over blend of a premultiplied BGRA image (what FreeType produces for
 * color glyphs) onto the destination.
 */
fn blend_premultiplied(mut dest: ImgRefMut<BGRA<u8>>, src: ImgRef<BGRA<u8>>)
{
    for (d_row, s_row) in iter::zip(dest.rows_mut(), src.rows()) {
        for (d, s) in iter::zip(d_row, s_row) {
            let inv = 255 - s.a as u32;
            d.b = (s.b as u32 + (d.b as u32 * inv + 127) / 255).min(255) as u8;
            d.g = (s.g as u32 + (d.g as u32 * inv + 127) / 255).min(255) as u8;
            d.r = (s.r as u32 + (d.r as u32 * inv + 127) / 255).min(255) as u8;
            d.a = (s.a as u32 + (d.a as u32 * inv + 127) / 255).min(255) as u8;
        }
    }
}

/*
 * Area-average (box filter) downscale of a premultiplied BGRA image. Averaging
 * premultiplied values is the correct way to filter alpha images. This is what
 * keeps emoji readable on small screens; nearest-neighbor looks broken at
 * 109px -> ~20px.
 */
fn downscale_bgra(src: ImgRef<BGRA<u8>>, dw: usize, dh: usize) -> ImgVec<BGRA<u8>>
{
    let (sw, sh) = (src.width(), src.height());
    let xr = sw as f32 / dw as f32;
    let yr = sh as f32 / dh as f32;
    let mut out = Vec::with_capacity(dw * dh);

    for dy in 0..dh {
        let y0 = dy as f32 * yr;
        let y1 = y0 + yr;

        for dx in 0..dw {
            let x0 = dx as f32 * xr;
            let x1 = x0 + xr;

            let mut acc = [0f32; 4];
            let mut area = 0f32;

            for sy in (y0 as usize)..(y1.ceil() as usize).min(sh) {
                let wy = y1.min(sy as f32 + 1.0) - y0.max(sy as f32);

                for sx in (x0 as usize)..(x1.ceil() as usize).min(sw) {
                    let w = wy * (x1.min(sx as f32 + 1.0) - x0.max(sx as f32));
                    let p = src[(sx, sy)];

                    acc[0] += p.b as f32 * w;
                    acc[1] += p.g as f32 * w;
                    acc[2] += p.r as f32 * w;
                    acc[3] += p.a as f32 * w;
                    area += w;
                }
            }

            let inv = if area > 0.0 { 1.0 / area } else { 0.0 };
            out.push(BGRA {
                b: (acc[0] * inv + 0.5) as u8,
                g: (acc[1] * inv + 0.5) as u8,
                r: (acc[2] * inv + 0.5) as u8,
                a: (acc[3] * inv + 0.5) as u8,
            });
        }
    }

    ImgVec::new(out, dw, dh)
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

/*
 * Decide whether a label should be drawn from a color emoji font. The gate is
 * deliberately narrow: color emoji fonts also contain digits, '#', '*', '(c)'
 * and some arrows (for keycap and text-style sequences), and those must keep
 * going through the normal monochrome path. None of the labels in LABELS fall
 * inside these ranges.
 */
fn is_emoji_text(text: &str) -> bool
{
    text.chars().any(|c| matches!(c as u32,
        0x1F000..=0x1FFFF   // pictographs, emoticons, transport, ...
        | 0x2600..=0x27BF   // misc symbols, dingbats
        | 0x2B1B | 0x2B1C | 0x2B50 | 0x2B55
        | 0xFE0F))          // emoji variation selector
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

enum Pixels {
    /// Coverage mask, tinted with the label color when drawn.
    Mask(ImgVec<u8>),
    /// Premultiplied BGRA, drawn as-is (color emoji).
    Color(ImgVec<BGRA<u8>>),
}

struct TextRaster {
    pixels: Pixels,
    y: usize,
}

impl TextRaster {
    fn width(&self) -> usize
    {
        match &self.pixels {
            Pixels::Mask(img) => img.width(),
            Pixels::Color(img) => img.width(),
        }
    }

    fn height(&self) -> usize
    {
        match &self.pixels {
            Pixels::Mask(img) => img.height(),
            Pixels::Color(img) => img.height(),
        }
    }
}

pub struct Graphics<D: Display> {
    disp: D,
    fonts: Vec<(String, freetype::FT_Long)>,
    /*
     * Lazily opened faces, one slot per entry in `fonts`. None: not opened
     * yet. Some(0): failed to open. Some(p): FT_Face pointer. Stored as usize
     * so the struct stays Send, like the AtomicPtr used for `ft`.
     */
    faces: Vec<Option<usize>>,
    /// None: not searched yet. Some(None): no color font installed.
    color_font: Option<Option<usize>>,
    labels: HashMap<String, TextRaster>,
    sublabels: HashMap<String, TextRaster>,
    /// (label, sublabel) font sizes the caches above were rendered for.
    raster_sizes: (u64, u64),
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

        let faces = fontfiles.iter().map(|_| None).collect();

        Graphics {
            disp,
            ft,
            fonts: fontfiles,
            faces,
            color_font: None,
            labels: HashMap::new(),
            sublabels: HashMap::new(),
            raster_sizes: (0, 0),

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

    /*
     * Open (once) and return the face for fonts[idx]. Faces are kept for the
     * lifetime of the FreeType library, which frees them in FT_Done_FreeType().
     *
     * The face is opened from the path rather than from a Vec<u8> read into
     * memory, so FreeType can mmap it. This matters for emoji fonts, which are
     * 10+ MB.
     */
    fn face(&mut self, idx: usize) -> Option<freetype::FT_Face>
    {
        if idx >= self.fonts.len() {
            return None;
        }

        if self.faces[idx].is_none() {
            let path = CString::new(self.fonts[idx].0.as_bytes()).ok()?;
            let mut face: freetype::FT_Face = ptr::null_mut();

            let err = unsafe {
                freetype::FT_New_Face(*self.ft.get_mut(),
                                      path.as_ptr(),
                                      self.fonts[idx].1,
                                      &mut face)
            };

            self.faces[idx] = Some(if err == 0 { face as usize } else { 0 });
        }

        match self.faces[idx] {
            Some(p) if p != 0 => Some(p as freetype::FT_Face),
            _ => None,
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
        for i in 0..self.fonts.len() {
            if let Some(face) = self.face(i) {
                if Self::can_font_render_text(Some(face), text) {
                    return Some(face);
                }
            }
        }

        // Fall back to the first usable font with placeholder glyphs.
        (0..self.fonts.len()).find_map(|i| self.face(i))
    }

    /// Index of the first installed font with color glyphs (e.g. Noto Color Emoji).
    fn color_font_idx(&mut self) -> Option<usize>
    {
        if self.color_font.is_none() {
            let found = (0..self.fonts.len()).find(|&i| {
                self.face(i).map_or(false, |face| unsafe {
                    ((*face).face_flags as i64) & FT_FACE_FLAG_COLOR != 0
                })
            });
            self.color_font = Some(found);
        }

        self.color_font.flatten()
    }

    /*
     * Rasterize a single color emoji to a premultiplied BGRA image of the
     * requested size. Returns None when the text is not a (single) emoji, no
     * color font is installed, or the glyph is not a color bitmap; the caller
     * then falls back to the monochrome path.
     *
     * Multi-codepoint sequences (skin tones, ZWJ families, flags) need text
     * shaping to select the right glyph, which FreeType alone cannot do, so
     * they are not handled here.
     */
    fn rasterize_color(&mut self, size: f64, text: &str) -> Option<TextRaster>
    {
        if !is_emoji_text(text) {
            return None;
        }

        // Variation selectors do not change which glyph the font has.
        let mut chars = text.chars().filter(|&c| c != '\u{FE0F}' && c != '\u{FE0E}');
        let ch = chars.next()?;
        if chars.next().is_some() {
            return None;
        }

        let idx = self.color_font_idx()?;
        let face = self.face(idx)?;
        let target = ((size * EMOJI_SCALE).round() as i32).max(8);

        let img = unsafe {
            if freetype::FT_Get_Char_Index(face, ch.into()) == 0 {
                return None;
            }

            if ((*face).face_flags as i64) & FT_FACE_FLAG_FIXED_SIZES != 0
                && (*face).num_fixed_sizes > 0
            {
                /*
                 * Bitmap fonts (CBDT/sbix) have fixed strikes and reject
                 * FT_Set_Pixel_Sizes() with "invalid pixel size". Select the
                 * smallest strike that is at least as big as the target (or
                 * the biggest one) and scale the result ourselves.
                 */
                let sizes = std::slice::from_raw_parts((*face).available_sizes,
                                                       (*face).num_fixed_sizes as usize);
                let strike = sizes.iter().enumerate()
                    .filter(|&(_, s)| s.height as i32 >= target)
                    .min_by_key(|&(_, s)| s.height)
                    .or_else(|| sizes.iter().enumerate().max_by_key(|&(_, s)| s.height))
                    .map(|(i, _)| i)?;

                if freetype::FT_Select_Size(face, strike as _) != 0 {
                    return None;
                }
            } else if freetype::FT_Set_Pixel_Sizes(face, 0, target as _) != 0 {
                return None;
            }

            if freetype::FT_Load_Char(face, ch.into(), FT_LOAD_COLOR | FT_LOAD_RENDER) != 0 {
                return None;
            }

            let bm = &(*(*face).glyph).bitmap;
            let (w, h, pitch) = (bm.width as usize, bm.rows as usize, bm.pitch as isize);

            if bm.pixel_mode != FT_PIXEL_MODE_BGRA
                || w == 0 || h == 0 || pitch <= 0 || bm.buffer.is_null()
            {
                return None;
            }

            let bytes = std::slice::from_raw_parts(bm.buffer, pitch as usize * h);
            let mut px = Vec::with_capacity(w * h);

            for row in bytes.chunks(pitch as usize) {
                for p in row[..w * 4].chunks_exact(4) {
                    px.push(BGRA { b: p[0], g: p[1], r: p[2], a: p[3] });
                }
            }

            ImgVec::new(px, w, h)
        };

        let (w, h) = (img.width(), img.height());
        let img = if h as i32 == target {
            img
        } else {
            let dh = target as usize;
            let dw = (w * dh / h).max(1);
            downscale_bgra(img.as_ref(), dw, dh)
        };

        Some(TextRaster {
            pixels: Pixels::Color(img),
            y: 0,
        })
    }

    fn rasterize_text(&mut self, size: f64, text: &str) -> TextRaster
    {
        let ftface = match self.open_font_for_text(text) {
            Some(face) => face,
            None => {
                // No usable font at all: draw nothing instead of panicking.
                return TextRaster {
                    pixels: Pixels::Mask(ImgVec::new(vec![0], 1, 1)),
                    y: 0,
                };
            }
        };

        unsafe {
            freetype::FT_Set_Char_Size(ftface, (size * 64.0) as i64, 0, 0, 0);
        };

        let (metrics, units_per_em, bbox) = unsafe {
            ((*(*ftface).size).metrics,
            (*ftface).units_per_EM,
            (*ftface).bbox)
        };

        let mut width = 0usize;
        let glyph_width = ((bbox.xMax - bbox.xMin) as usize
                        * metrics.x_ppem as usize)
                        .div_ceil(units_per_em as usize);

        let stride = if text.is_empty() {
            1
        } else {
            /*
             * Upper bound on the pen position plus the last glyph's box. The
             * font-wide bbox/max_advance are only estimates, so be generous
             * (n advances, not n - 1) and clamp `width` below.
             */
            let n = text.chars().count();
            (metrics.max_advance as usize / 64 + 1) * n + glyph_width + 2
        };

        let y_max = cmp::max(metrics.ascender as isize / 64,
                             bbox.yMax as isize
                           * metrics.y_ppem as isize
                           / units_per_em as isize);
        let y_min = cmp::min(metrics.descender as isize / 64,
                             bbox.yMin as isize
                           * metrics.y_ppem as isize
                           / units_per_em as isize);
        let mut y = (y_max - metrics.ascender as isize / 64).max(0) as usize;
        let mut height = (y_max - y_min).max(0) as usize;

        let vec: Vec<u8> = vec![0; stride * height];
        let mut img = ImgVec::new(vec, stride, height);

        let mut pen = 0isize;

        for c in text.chars() {
            let glyph = unsafe {
                freetype::FT_Load_Char(ftface, c.into(), 0);
                freetype::FT_Render_Glyph((*ftface).glyph, freetype::FT_Render_Mode::FT_RENDER_MODE_NORMAL);
                *(*ftface).glyph
            };

            if glyph.bitmap.rows != 0 && glyph.bitmap.width != 0 {
                let bitmap_left = glyph.bitmap_left as isize;
                pen = pen.max(-bitmap_left);

                let dest_x = (pen + bitmap_left).max(0) as usize;
                let dest_y = (y_max - glyph.bitmap_top as isize).max(0) as usize;

                let src = bitmap_to_imgref(&glyph.bitmap);
                let draw_w = (glyph.bitmap.width as usize)
                    .min(img.width().saturating_sub(dest_x));
                let draw_h = (glyph.bitmap.rows as usize)
                    .min(img.height().saturating_sub(dest_y));

                if draw_w != 0 && draw_h != 0 {
                    y = y.min(dest_y);
                    height = height.max(dest_y + draw_h);

                    let src = src.sub_image(0, 0, draw_w, draw_h);
                    let dest = img.sub_image_mut(dest_x, dest_y, draw_w, draw_h);
                    copy_image(dest, src);
                }
            }

            let new_width = (pen + glyph.bitmap_left as isize + glyph.bitmap.width as isize)
                .max(0) as usize;
            width = width.max(new_width);

            pen += glyph.advance.x as isize / 64;
        }

        // The face is cached and owned by FreeType; do not FT_Done_Face() it.

        if height == 0 {
            height = 1;
        }
        if width == 0 {
            width = 1;
        }
        // imgref requires stride >= width.
        width = width.min(stride.max(1));

        let mut vec = img.into_buf();
        let needed = stride.saturating_mul(height);
        vec.resize(needed, 0); // truncates or zero-pads
        let img = ImgVec::new_stride(vec, width, height, stride);

        TextRaster {
            pixels: Pixels::Mask(img),
            y,
        }
    }

    fn rasterize_any(&mut self, size: f64, label: &str) -> TextRaster
    {
        let size = if Self::is_label_small(label) {
            size * 0.75
        } else {
            size
        };

        match self.rasterize_color(size, label) {
            Some(raster) => raster,
            None => self.rasterize_text(size, label),
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
            let img = self.rasterize_any(size, label);
            self.labels.insert(String::from(label), img);
        }
    }

    /* Same as rasterize_label(), for the sublabels hash table. */
    fn rasterize_sublabel(&mut self, size: f64, label: &str)
    {
        if !self.sublabels.contains_key(label) {
            let img = self.rasterize_any(size, label);
            self.sublabels.insert(String::from(label), img);
        }
    }

    fn rasterize_labels(&mut self, layout: &Layout)
    {
        let label_size = self.y_scale * 0.33;
        let sublabel_size = self.y_scale * 0.22;

        /*
         * Keep rasterized labels across layout changes and only throw them
         * away when the font size changed. Re-rasterizing (and especially
         * scaling emoji) is the expensive part.
         */
        let sizes = (label_size.to_bits(), sublabel_size.to_bits());
        if self.raster_sizes != sizes {
            self.labels.clear();
            self.sublabels.clear();
            self.raster_sizes = sizes;
        }

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

        let full_height = src.height();
        let width = src.width();
        let height = full_height.saturating_sub(src.y);

        let x = match x_anchor {
            Anchor::Min => SUBLABEL_MARGIN,
            Anchor::Center => (dest.width().saturating_sub(width)) / 2,
            Anchor::Max => dest.width().saturating_sub(width + SUBLABEL_MARGIN),
        };

        let y = match y_anchor {
            Anchor::Min => SUBLABEL_MARGIN,
            Anchor::Center => (dest.height().saturating_sub(height)) / 2,
            Anchor::Max => dest.height().saturating_sub(height + SUBLABEL_MARGIN),
        };

        let src_y = src.y.min(full_height);
        let draw_w = width.min(dest.width().saturating_sub(x));
        let draw_h = height.min(dest.height().saturating_sub(y));

        if draw_w == 0 || draw_h == 0 {
            return;
        }

        let dest = dest.sub_image_mut(x, y, draw_w, draw_h);

        match &src.pixels {
            Pixels::Mask(img) => {
                let src = img.sub_image(0, src_y, draw_w, draw_h);
                convert_gray_to_bgrx(dest, src, fg_color);
            }
            Pixels::Color(img) => {
                let src = img.sub_image(0, src_y, draw_w, draw_h);
                blend_premultiplied(dest, src);
            }
        }
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
        let x1 = ((key.x1 * x_scale) as usize).saturating_add(keycap_padding);
        let y1 = ((key.y1 * y_scale) as usize).saturating_add(keycap_padding);
        let x2 = ((key.x2 * x_scale) as usize).saturating_sub(keycap_padding);
        let y2 = ((key.y2 * y_scale) as usize).saturating_sub(keycap_padding);

        let width = x2.saturating_sub(x1);
        let height = y2.saturating_sub(y1);

        if width == 0 || height == 0 {
            return;
        }

        let mut keyimg = img.sub_image_mut(x1, y1, width, height);

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

        self.disp.end(x1, y1, x2.saturating_sub(x1), y2.saturating_sub(y1));
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
        // Also frees all faces opened through face().
        unsafe {
            freetype::FT_Done_FreeType(*self.ft.get_mut());
        }
    }
}
