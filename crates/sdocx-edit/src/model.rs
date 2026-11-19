//! Editable note model.
//!
//! Coordinates are Samsung Notes page units (pixels of the page canvas, origin at
//! the page's top-left corner). Elements that came from a file keep their original
//! serialized record in [`RawObject`]; as long as an element is not changed it is
//! written back byte-for-byte, so pen types and features this editor does not
//! understand survive a save.

use std::sync::Arc;

/// Colour as `0xAARRGGBB`.
pub type Argb = u32;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    pub fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            left: left.min(right),
            top: top.min(bottom),
            right: left.max(right),
            bottom: top.max(bottom),
        }
    }
    pub fn width(&self) -> f32 {
        self.right - self.left
    }
    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }
    pub fn union(self, o: Rect) -> Rect {
        Rect {
            left: self.left.min(o.left),
            top: self.top.min(o.top),
            right: self.right.max(o.right),
            bottom: self.bottom.max(o.bottom),
        }
    }
    pub fn expand(self, d: f32) -> Rect {
        Rect { left: self.left - d, top: self.top - d, right: self.right + d, bottom: self.bottom + d }
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.left <= o.right && o.left <= self.right && self.top <= o.bottom && o.top <= self.bottom
    }
    pub fn translate(self, dx: f32, dy: f32) -> Rect {
        Rect { left: self.left + dx, top: self.top + dy, right: self.right + dx, bottom: self.bottom + dy }
    }
}

/// A serialized page object record copied verbatim from the source file
/// (type byte, child count, size, payload, hash and any child records).
#[derive(Debug, Clone)]
pub struct RawObject {
    pub bytes: Arc<Vec<u8>>,
    /// Integrity trailers of this record and its children in depth-first order.
    pub hashes: Vec<[u8; 32]>,
    /// Samsung object type id.
    pub object_type: u8,
}

#[derive(Debug, Clone)]
pub struct Stroke {
    pub points: Vec<[f32; 2]>,
    /// Pen pressure per point, 0..=1.
    pub pressures: Vec<f32>,
    pub color: Argb,
    /// Pen width in page units.
    pub width: f32,
    pub raw: Option<RawObject>,
}

impl Stroke {
    pub fn bounds(&self) -> Rect {
        let mut r = Rect { left: f32::MAX, top: f32::MAX, right: f32::MIN, bottom: f32::MIN };
        for p in &self.points {
            r.left = r.left.min(p[0]);
            r.top = r.top.min(p[1]);
            r.right = r.right.max(p[0]);
            r.bottom = r.bottom.max(p[1]);
        }
        if self.points.is_empty() {
            return Rect::default();
        }
        r.expand(self.width / 2.0)
    }
}

#[derive(Debug, Clone)]
pub struct TextBox {
    pub rect: Rect,
    pub text: String,
    /// Font size in Samsung units (multiply by [`Note::density`] for page units).
    pub font_size: f32,
    pub color: Argb,
    pub raw: Option<RawObject>,
}

#[derive(Debug, Clone)]
pub struct Image {
    pub rect: Rect,
    pub data: Arc<Vec<u8>>,
    /// File name used inside the archive's `media/` folder (without the bind-id prefix).
    pub name: String,
    pub raw: Option<RawObject>,
    /// Media bind id of the original file, when the element came from a note.
    pub source_bind_id: Option<u32>,
}

/// An object this editor cannot edit (shapes, tables, formulas, voice, web cards…).
/// It is drawn by the high-fidelity renderer and written back unchanged.
#[derive(Debug, Clone)]
pub struct Opaque {
    pub bounds: Rect,
    pub kind: String,
    pub raw: RawObject,
    /// Hidden objects (e.g. on inactive layers) are kept but not selectable.
    pub hidden: bool,
}

#[derive(Debug, Clone)]
pub enum Element {
    Stroke(Stroke),
    Text(TextBox),
    Image(Image),
    Opaque(Opaque),
}

impl Element {
    pub fn bounds(&self) -> Rect {
        match self {
            Element::Stroke(s) => s.bounds(),
            Element::Text(t) => t.rect,
            Element::Image(i) => i.rect,
            Element::Opaque(o) => o.bounds,
        }
    }
    pub fn raw(&self) -> Option<&RawObject> {
        match self {
            Element::Stroke(s) => s.raw.as_ref(),
            Element::Text(t) => t.raw.as_ref(),
            Element::Image(i) => i.raw.as_ref(),
            Element::Opaque(o) => Some(&o.raw),
        }
    }
    /// Marks the element as changed so it is re-encoded on save.
    pub fn touch(&mut self) {
        match self {
            Element::Stroke(s) => s.raw = None,
            Element::Text(t) => t.raw = None,
            Element::Image(i) => i.raw = None,
            Element::Opaque(_) => {}
        }
    }
    pub fn translate(&mut self, dx: f32, dy: f32) {
        match self {
            Element::Stroke(s) => {
                for p in &mut s.points {
                    p[0] += dx;
                    p[1] += dy;
                }
            }
            Element::Text(t) => t.rect = t.rect.translate(dx, dy),
            Element::Image(i) => i.rect = i.rect.translate(dx, dy),
            Element::Opaque(_) => return,
        }
        self.touch();
    }
    pub fn is_movable(&self) -> bool {
        !matches!(self, Element::Opaque(_))
    }
}

/// The original bytes of a page, kept so an untouched page is saved unchanged.
#[derive(Debug, Clone)]
pub struct PageSource {
    pub bytes: Arc<Vec<u8>>,
    /// Hash recorded for this page in `pageIdInfo.dat` (or its own trailer).
    pub page_hash: [u8; 32],
    pub layer_offset: usize,
    /// Byte ranges of the layers that were not decoded into elements, with their
    /// header offsets, re-emitted unchanged after the edited layer.
    pub layer_count: usize,
    pub current_layer: usize,
    /// (start, end) byte range of every layer record.
    pub layer_ranges: Vec<(usize, usize)>,
    /// Layer identity (uuid, time) of the current layer when decodable.
    pub current_layer_identity: Option<(String, u64)>,
    /// Page identity (uuid, modified time).
    pub identity: (String, u64),
}

#[derive(Debug, Clone)]
pub struct Page {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub background: Argb,
    pub elements: Vec<Element>,
    pub source: Option<PageSource>,
    /// Set once any element on the page is added, removed or changed.
    pub dirty: bool,
    /// The page has content this editor cannot draw itself (PDF paper, template).
    pub has_special_background: bool,
}

impl Page {
    pub fn blank(width: u32, height: u32, background: Argb) -> Self {
        Self {
            id: crate::bin::samsung_uuid(),
            width,
            height,
            background,
            elements: Vec::new(),
            source: None,
            dirty: true,
            has_special_background: false,
        }
    }
}

/// Media file carried over from the source archive.
#[derive(Debug, Clone)]
pub struct MediaFile {
    pub bind_id: u32,
    pub file_name: String,
    pub data: Arc<Vec<u8>>,
    /// Original `mediaInfo.dat` entry body (after the size field), re-emitted unchanged.
    pub raw_entry: Option<Vec<u8>>,
}

/// Everything from the source archive needed to save with minimal changes.
#[derive(Debug, Clone, Default)]
pub struct NoteSource {
    /// All archive entries by name (relative to the note root).
    pub entries: Vec<(String, Arc<Vec<u8>>)>,
    pub note_bytes: Option<Arc<Vec<u8>>>,
    pub end_tag: Option<Arc<Vec<u8>>>,
    pub media: Vec<MediaFile>,
    pub media_info_version: Option<u32>,
    pub original_title: String,
    pub original_body: String,
    /// `note.note` header width, height and vertical page padding.
    pub note_dims: Option<(u32, u32, u32)>,
    pub encrypted: bool,
}

#[derive(Debug, Clone)]
pub struct Note {
    pub title: String,
    /// Keyboard text of the note body (flows over the pages in Samsung Notes).
    pub body: String,
    pub pages: Vec<Page>,
    /// Samsung text sizes are multiplied by this (page width / 360) to get page units.
    pub density: f32,
    pub source: Option<Arc<NoteSource>>,
    /// Problems noticed while importing.
    pub warnings: Vec<String>,
}

impl Default for Note {
    fn default() -> Self {
        Self::new()
    }
}

impl Note {
    pub const DEFAULT_WIDTH: u32 = 1080;
    pub const DEFAULT_HEIGHT: u32 = 1527;
    pub const DEFAULT_BACKGROUND: Argb = 0xFFFC_FCFC;

    pub fn new() -> Self {
        Self {
            title: String::new(),
            body: String::new(),
            pages: vec![Page::blank(Self::DEFAULT_WIDTH, Self::DEFAULT_HEIGHT, Self::DEFAULT_BACKGROUND)],
            density: Self::DEFAULT_WIDTH as f32 / 360.0,
            source: None,
            warnings: Vec::new(),
        }
    }

    pub fn text_changed(&self) -> bool {
        match &self.source {
            Some(s) => s.original_title != self.title || s.original_body != self.body,
            None => true,
        }
    }
}
