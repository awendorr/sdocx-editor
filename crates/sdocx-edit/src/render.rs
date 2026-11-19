//! High-fidelity page rendering through the `sdocx` crate's SVG renderer.

use crate::Error;
use crate::model::{Element, Note};

/// What to include when rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// Everything, exactly as the note would be saved.
    Full,
    /// Background, note text and objects this editor cannot edit. The editor
    /// draws strokes, text boxes and images on top of this itself.
    Underlay,
}

#[derive(Debug, Clone)]
pub struct PageSvg {
    /// Index into [`Note::pages`].
    pub page: usize,
    pub width: u32,
    pub height: u32,
    pub svg: String,
}

fn fonts() -> &'static sdocx::fonts::FontBook {
    static FONTS: std::sync::OnceLock<sdocx::fonts::FontBook> = std::sync::OnceLock::new();
    FONTS.get_or_init(sdocx::fonts::FontBook::default)
}

/// Renders the note's pages to standalone SVG documents.
pub fn render_svgs(note: &Note, mode: RenderMode) -> Result<Vec<PageSvg>, Error> {
    render_pages(note, mode, None)
}

/// Renders only the pages whose indices are listed (all when `None`).
pub fn render_pages(
    note: &Note,
    mode: RenderMode,
    pages: Option<&[usize]>,
) -> Result<Vec<PageSvg>, Error> {
    let bytes = match mode {
        RenderMode::Full => crate::save_bytes(note)?.0,
        RenderMode::Underlay => {
            // Untouched original objects are drawn faithfully by the sdocx
            // renderer; the editor paints new and changed ones itself.
            let mut n = note.clone();
            for p in &mut n.pages {
                let before = p.elements.len();
                p.elements
                    .retain(|e| e.raw().is_some() && !matches!(e, Element::Opaque(o) if o.hidden));
                p.dirty |= p.elements.len() != before;
            }
            crate::save_bytes(&n)?.0
        }
    };
    let doc = sdocx::parse_bytes(&bytes)
        .map_err(|e| Error::Format(format!("Önizleme oluşturulamadı: {e}")))?;
    let options = sdocx::RenderOptions::default();
    let layout = sdocx::layout_document(&doc);
    let mut cache = sdocx::DocumentTextCache::default();
    let mut out = Vec::new();
    for (visible, lp) in layout.pages.iter().enumerate() {
        let index = lp.source_page_index;
        if index >= note.pages.len() || pages.is_some_and(|p| !p.contains(&index)) {
            continue;
        }
        if let Some(r) = cache.render_layout_page_svg(&doc, &layout, visible, &options, fonts()) {
            out.push(PageSvg {
                page: index,
                width: r.width,
                height: r.height,
                svg: r.svg,
            });
        }
    }
    Ok(out)
}

/// Rasterises SVG pages with the fonts bundled in the `sdocx` crate.
pub struct Rasterizer {
    options: resvg::usvg::Options<'static>,
}

impl Default for Rasterizer {
    fn default() -> Self {
        let fonts = sdocx::fonts::FontBook::default();
        Self {
            options: resvg::usvg::Options {
                font_resolver: sdocx::fonts::svg_font_resolver(&fonts.database()),
                fontdb: fonts.database(),
                ..Default::default()
            },
        }
    }
}

/// An RGBA image (premultiplied alpha converted to straight alpha).
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Rasterizer {
    /// Renders `svg` scaled by `scale`; sides are capped at `max_side` pixels.
    pub fn rasterize(&self, svg: &str, scale: f32, max_side: u32) -> Result<RgbaImage, Error> {
        let tree = resvg::usvg::Tree::from_str(svg, &self.options)
            .map_err(|e| Error::Format(format!("SVG okunamadı: {e}")))?;
        let size = tree.size();
        let fit = (max_side as f32 / size.width().max(size.height())).min(scale);
        let w = (size.width() * fit).ceil().max(1.0) as u32;
        let h = (size.height() * fit).ceil().max(1.0) as u32;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)
            .ok_or_else(|| Error::Format("Bellek ayrılamadı".into()))?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(fit, fit),
            &mut pixmap.as_mut(),
        );
        let mut pixels = pixmap.take();
        for px in pixels.chunks_exact_mut(4) {
            let a = px[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut px[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        Ok(RgbaImage {
            width: w,
            height: h,
            pixels,
        })
    }
}

/// Exports the note to a vector PDF with the `sdocx` crate's PDF renderer.
pub fn export_pdf(note: &Note) -> Result<Vec<u8>, Error> {
    let (bytes, _) = crate::save_bytes(note)?;
    let doc = sdocx::parse_bytes(&bytes)
        .map_err(|e| Error::Format(format!("PDF oluşturulamadı: {e}")))?;
    sdocx::render_document_pdf(
        &doc,
        &sdocx::RenderOptions::default(),
        &sdocx::PdfOptions::default(),
    )
    .map_err(|e| Error::Format(format!("PDF oluşturulamadı: {e}")))
}
