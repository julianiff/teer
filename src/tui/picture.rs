//! The Visualize picture inside the terminal: teer's own SVG turned into
//! pixels (resvg, with the system's fonts) and shown with the terminal's
//! graphics protocol (Kitty, iTerm2 or Sixel, through ratatui-image).
//! Terminals without one keep the text list.

use std::cell::RefCell;
use std::sync::{Arc, OnceLock};

use image::{DynamicImage, RgbaImage};
use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{Image, Resize};
use resvg::usvg::fontdb;
use resvg::{tiny_skia, usvg};

use crate::review::model::Visual;
use crate::review::visual::{self, Paint, Palette};

/// Terminal text fills about this share of a cell's height; the picture's
/// 13px labels are scaled to match, unless it has to shrink to fit.
const TEXT_SHARE: f32 = 0.7;
const LABEL_PX: f32 = 13.0;

/// What the terminal can show, and the picture last drawn (one at a time:
/// it is turned into pixels again only when the picture, the area or the
/// colours change).
pub struct Pictures {
    picker: Picker,
    shown: RefCell<Option<Shown>>,
}

struct Shown {
    visual: Visual,
    size: Size,
    palette: Palette,
    protocol: Protocol,
}

impl Pictures {
    /// Ask the terminal whether it can show images. Must run in raw mode
    /// before anything reads terminal events. `None` without a graphics
    /// protocol (half blocks are too coarse for labels).
    pub fn detect() -> Option<Pictures> {
        let picker = Picker::from_query_stdio().ok()?;
        if picker.protocol_type() == ProtocolType::Halfblocks {
            return None;
        }
        // Loading the system's fonts takes a moment: do it before it is needed.
        std::thread::spawn(fonts);
        Some(Pictures::new(picker))
    }

    pub fn new(picker: Picker) -> Pictures {
        Pictures {
            picker,
            shown: RefCell::new(None),
        }
    }

    /// Draw `v` at the top left of `area`. False when it could not be drawn
    /// (the caller shows the text instead).
    pub fn draw(&self, f: &mut Frame, area: Rect, v: &Visual, palette: &'static Palette) -> bool {
        if area.width == 0 || area.height == 0 {
            return false;
        }
        let size = area.as_size();
        let mut shown = self.shown.borrow_mut();
        let stale = shown
            .as_ref()
            .is_none_or(|s| s.visual != *v || s.size != size || s.palette != *palette);
        if stale {
            let cell = self.picker.font_size();
            let fit = (
                f32::from(size.width) * f32::from(cell.width),
                f32::from(size.height) * f32::from(cell.height),
            );
            let protocol = rasterize(v, palette, fit, f32::from(cell.height))
                .and_then(|img| self.picker.new_protocol(img, size, Resize::Fit(None)).ok());
            *shown = protocol.map(|protocol| Shown {
                visual: v.clone(),
                size,
                palette: *palette,
                protocol,
            });
        }
        match shown.as_ref() {
            Some(s) => {
                f.render_widget(Image::new(&s.protocol), area);
                true
            }
            None => false,
        }
    }
}

/// The system's fonts, loaded once. `sans-serif` is Arial by default, which
/// not every system has, so it is pointed at the first common one found.
pub fn fonts() -> Arc<fontdb::Database> {
    static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            let has = |name: &str| {
                db.faces()
                    .any(|f| f.families.iter().any(|(n, _)| n == name))
            };
            let sans = [
                "Helvetica Neue",
                "Helvetica",
                "Arial",
                "DejaVu Sans",
                "Liberation Sans",
                "Noto Sans",
            ]
            .into_iter()
            .find(|n| has(n));
            if let Some(name) = sans {
                db.set_sans_serif_family(name);
            }
            Arc::new(db)
        })
        .clone()
}

fn color(hex: &str) -> tiny_skia::Color {
    let h = hex.trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    tiny_skia::Color::from_rgba8(byte(0), byte(2), byte(4), 255)
}

/// `v` in `palette`'s colours as pixels, at most `fit` (width, height) pixels,
/// with labels about as tall as terminal text in cells `cell_h` pixels high.
pub fn rasterize(
    v: &Visual,
    palette: &'static Palette,
    fit: (f32, f32),
    cell_h: f32,
) -> Option<DynamicImage> {
    let svg = visual::svg(v, Paint::Fixed(palette));
    let opts = usvg::Options {
        fontdb: fonts(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(&svg, &opts).ok()?;
    let (w, h) = (tree.size().width(), tree.size().height());
    let scale = (TEXT_SHARE * cell_h / LABEL_PX)
        .min(fit.0 / w)
        .min(fit.1 / h);
    if !(scale.is_finite() && scale > 0.0) {
        return None;
    }
    let (pw, ph) = ((w * scale).floor() as u32, (h * scale).floor() as u32);
    let mut pixmap = tiny_skia::Pixmap::new(pw.max(1), ph.max(1))?;
    // Opaque, so tiny-skia's premultiplied pixels are plain RGBA.
    pixmap.fill(color(palette.bg));
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let (pw, ph) = (pixmap.width(), pixmap.height());
    RgbaImage::from_raw(pw, ph, pixmap.take()).map(DynamicImage::ImageRgba8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{Cite, Delta, VisualEdge, VisualNode};
    use crate::review::visual::{DARK, LIGHT};

    fn picture() -> Visual {
        let ev = vec![Cite {
            file: "a.rs".into(),
            start: 1,
            end: 1,
        }];
        let node = |id: &str, status| VisualNode {
            id: id.into(),
            label: id.into(),
            status,
            evidence: ev.clone(),
        };
        Visual {
            claim: "f now writes to the cache.".into(),
            nodes: vec![node("f", Delta::Changed), node("cache", Delta::Added)],
            edges: vec![VisualEdge {
                from: "f".into(),
                to: "cache".into(),
                label: "writes".into(),
                status: Delta::Added,
                evidence: ev,
            }],
            path: "/d/a.html".into(),
        }
    }

    fn has(img: &RgbaImage, hex: &str) -> bool {
        let c = color(hex).to_color_u8();
        let want = [c.red(), c.green(), c.blue()];
        img.pixels()
            .any(|p| p.0[..3].iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 8))
    }

    #[test]
    fn rasterizes_in_the_palette_and_within_the_fit() {
        let img = rasterize(&picture(), &DARK, (2000.0, 1000.0), 34.0)
            .unwrap()
            .to_rgba8();
        assert!(img.width() <= 2000 && img.height() <= 1000);
        assert!(
            img.width() > 300,
            "scaled up to terminal text: {}",
            img.width()
        );
        assert_eq!(
            img.get_pixel(0, 0).0,
            [0x0d, 0x11, 0x17, 255],
            "dark background"
        );
        assert!(has(&img, DARK.accent), "the added arrow");
        assert!(has(&img, DARK.changed), "the changed box");
        let light = rasterize(&picture(), &LIGHT, (2000.0, 1000.0), 34.0)
            .unwrap()
            .to_rgba8();
        assert_eq!(light.get_pixel(0, 0).0, [255, 255, 255, 255]);
        assert!(has(&light, LIGHT.accent));
    }

    #[test]
    fn a_small_area_shrinks_the_picture_to_fit() {
        let img = rasterize(&picture(), &DARK, (200.0, 60.0), 34.0).unwrap();
        assert!(
            img.width() <= 200 && img.height() <= 60,
            "{}x{}",
            img.width(),
            img.height()
        );
        assert!(rasterize(&picture(), &DARK, (0.0, 60.0), 34.0).is_none());
    }

    #[test]
    fn labels_are_drawn_with_a_system_font() {
        // With no font the labels would be skipped and only the shapes drawn.
        let db = fonts();
        assert!(!db.is_empty(), "no system fonts found");
        let with_text = rasterize(&picture(), &DARK, (2000.0, 1000.0), 34.0)
            .unwrap()
            .to_rgba8();
        let mut bare = picture();
        for n in &mut bare.nodes {
            n.label = " ".into();
        }
        for e in &mut bare.edges {
            e.label = " ".into();
        }
        let without = rasterize(&bare, &DARK, (2000.0, 1000.0), 34.0)
            .unwrap()
            .to_rgba8();
        let fg = |img: &RgbaImage| {
            img.pixels()
                .filter(|p| p.0[0] > 0xc0 && p.0[1] > 0xc0 && p.0[2] > 0xc0)
                .count()
        };
        assert!(fg(&with_text) > fg(&without) + 50);
    }
}
