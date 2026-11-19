use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Frame, Id, Key, KeyboardShortcut, Margin,
    Modifiers, Pos2, Rect, Sense, Shadow, Stroke as EStroke, StrokeKind, UiBuilder, Vec2,
};
use sdocx_edit::{self as model, Element, Note, Page, RenderMode};

use crate::icons::{self, Icon};
use crate::theme::{self, Palette};
use crate::worker::{Job, Output, TexKey, Worker};

/// Gap between pages in page units.
const PAGE_GAP: f32 = 40.0;
/// Rendered stroke width relative to the stored pen width (measured with the sdocx renderer).
const PEN_DISPLAY_FACTOR: f32 = 0.5;
/// Extra room below the last page for the "add page" button, in page units.
const ADD_PAGE_SPACE: f32 = 160.0;

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// Pen colours offered in the palette (6 per row).
const PALETTE: [[u8; 3]; 30] = [
    [0x20, 0x20, 0x22],
    [0x5f, 0x63, 0x68],
    [0x9a, 0xa0, 0xa6],
    [0xd0, 0xd3, 0xd7],
    [0xff, 0xff, 0xff],
    [0x8d, 0x5a, 0x3b],
    [0xe5, 0x39, 0x35],
    [0xfb, 0x8c, 0x00],
    [0xfd, 0xd8, 0x35],
    [0x43, 0xa0, 0x47],
    [0x00, 0x89, 0x7b],
    [0x1e, 0x88, 0xe5],
    [0x39, 0x49, 0xab],
    [0x8e, 0x24, 0xaa],
    [0xd8, 0x1b, 0x60],
    [0x4f, 0xc3, 0xf7],
    [0xc0, 0xca, 0x33],
    [0xff, 0xb3, 0x00],
    [0x0d, 0x2b, 0x5e],
    [0x1b, 0x5e, 0x20],
    [0x7f, 0x1d, 0x1d],
    [0xb3, 0x9d, 0xdb],
    [0xff, 0xab, 0x91],
    [0xa5, 0xd6, 0xa7],
    [0xf4, 0x8f, 0xb1],
    [0x80, 0xcb, 0xc4],
    [0xff, 0xe0, 0x82],
    [0x90, 0xa4, 0xae],
    [0x6d, 0x4c, 0x41],
    [0x26, 0x32, 0x38],
];

const HIGHLIGHT_PALETTE: [[u8; 3]; 6] = [
    [0xff, 0xe6, 0x00],
    [0x7c, 0xe0, 0x5a],
    [0x4d, 0xd0, 0xe1],
    [0xff, 0x80, 0xab],
    [0xff, 0xa7, 0x26],
    [0xb3, 0x88, 0xff],
];

const PEN_SIZES: [f32; 5] = [1.5, 3.0, 6.0, 10.0, 18.0];
const HIGHLIGHT_SIZES: [f32; 5] = [16.0, 28.0, 44.0, 64.0, 90.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Tool {
    Select,
    Pen,
    Highlighter,
    Eraser,
    Text,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    pub pen_color: [u8; 4],
    pub pen_width: f32,
    pub highlighter_color: [u8; 4],
    pub highlighter_width: f32,
    pub text_size: f32,
    pub text_color: [u8; 4],
    pub eraser_radius: f32,
    pub last_dir: Option<PathBuf>,
    pub dark: bool,
    pub pen_recent: Vec<[u8; 4]>,
    pub highlight_recent: Vec<[u8; 4]>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            pen_color: [32, 32, 34, 255],
            pen_width: 3.0,
            highlighter_color: [255, 230, 0, 110],
            highlighter_width: 28.0,
            text_size: 10.0,
            text_color: [32, 32, 34, 255],
            eraser_radius: 12.0,
            last_dir: None,
            dark: false,
            pen_recent: vec![
                [32, 32, 34, 255],
                [30, 136, 229, 255],
                [229, 57, 53, 255],
                [67, 160, 71, 255],
                [251, 140, 0, 255],
            ],
            highlight_recent: HIGHLIGHT_PALETTE
                .iter()
                .take(5)
                .map(|c| [c[0], c[1], c[2], 110])
                .collect(),
        }
    }
}

fn push_recent(list: &mut Vec<[u8; 4]>, c: [u8; 4]) {
    list.retain(|x| *x != c);
    list.insert(0, c);
    list.truncate(5);
}

#[derive(Clone)]
struct Snapshot {
    pages: Vec<Page>,
    title: String,
    body: String,
}

struct PageTex {
    tex: egui::TextureHandle,
    key: TexKey,
}

enum Pending {
    New,
    Open(Option<PathBuf>),
    Quit,
}

pub struct EditorApp {
    note: Note,
    path: Option<PathBuf>,
    /// The note was opened from `path` and has not been saved by us yet.
    opened_not_saved: bool,
    modified: bool,
    settings: Settings,
    tool: Tool,
    zoom: f32,
    fit_pending: bool,
    preview: bool,
    selection: Option<(usize, usize)>,
    /// Text box being typed into, edited in place on the page.
    editing_text: Option<(usize, usize)>,
    current_page: usize,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Snapshot taken when a text edit started (pushed on first change).
    text_edit_snapshot: Option<Snapshot>,
    // Drawing state
    drawing: Option<(usize, Vec<[f32; 2]>)>,
    dragging: Option<(usize, usize, Pos2)>,
    erasing: bool,
    // Rendering
    worker: Worker,
    textures: HashMap<String, PageTex>,
    thumbs: HashMap<String, PageTex>,
    page_versions: HashMap<String, u64>,
    /// Strokes erased from the rendered underlay whose page texture has not been
    /// redrawn yet: painted over in the page colour so erasing feels instant.
    erase_masks: HashMap<String, Vec<(u64, model::Stroke)>>,
    text_version: u64,
    version_counter: u64,
    image_cache: HashMap<usize, egui::TextureHandle>,
    status: String,
    shown_status: String,
    status_time: f64,
    errors: Vec<String>,
    show_info: bool,
    show_about: bool,
    show_pages: bool,
    show_body: bool,
    tool_popup: bool,
    /// Screen rectangle of the selected element, for the floating action bar.
    selection_anchor: Option<Rect>,
    pending: Option<Pending>,
    scroll_to_page: Option<usize>,
    focus_text: bool,
}

fn c32(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

fn argb(c: [u8; 4]) -> u32 {
    (c[3] as u32) << 24 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

fn from_argb(v: u32) -> Color32 {
    Color32::from_rgba_unmultiplied((v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8)
}

fn dist_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (x, y) = (a[0] + t * dx, a[1] + t * dy);
    ((p[0] - x).powi(2) + (p[1] - y).powi(2)).sqrt()
}

fn stroke_hit(s: &model::Stroke, p: [f32; 2], radius: f32) -> bool {
    if !s.bounds().expand(radius).contains(p[0], p[1]) {
        return false;
    }
    let r = radius + s.width / 2.0;
    match s.points.len() {
        0 => false,
        1 => dist_to_segment(p, s.points[0], s.points[0]) <= r,
        _ => s
            .points
            .windows(2)
            .any(|w| dist_to_segment(p, w[0], w[1]) <= r),
    }
}

// ---------- small widgets ----------

/// Square icon button; `active` gives it the accent background.
fn icon_button(
    ui: &mut egui::Ui,
    pal: &Palette,
    icon: Icon,
    active: bool,
    tip: &str,
    tint: Option<Color32>,
) -> egui::Response {
    icon_button_sized(ui, pal, icon, active, tip, tint, 38.0)
}

fn icon_button_sized(
    ui: &mut egui::Ui,
    pal: &Palette,
    icon: Icon,
    active: bool,
    tip: &str,
    tint: Option<Color32>,
    size: f32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let enabled = ui.is_enabled();
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if active {
            p.rect_filled(rect, CornerRadius::same(10), pal.accent_soft);
        } else if resp.hovered() && enabled {
            p.rect_filled(rect, CornerRadius::same(10), pal.surface_alt);
        }
        let mut fg = if active { pal.accent } else { pal.text };
        if !enabled {
            fg = pal.weak.gamma_multiply(0.6);
        }
        icons::paint(p, rect.shrink(size * 0.22), icon, fg, tint.unwrap_or(fg));
    }
    resp.on_hover_text(tip)
}

/// Round colour swatch; `selected` draws a ring around it.
fn swatch(
    ui: &mut egui::Ui,
    pal: &Palette,
    color: Color32,
    selected: bool,
    size: f32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let r = size / 2.0 - 4.0;
        if selected {
            p.circle_stroke(
                rect.center(),
                size / 2.0 - 1.0,
                EStroke::new(2.0, pal.accent),
            );
        } else if resp.hovered() {
            p.circle_stroke(
                rect.center(),
                size / 2.0 - 1.0,
                EStroke::new(1.5, pal.border),
            );
        }
        // Checkerboard hint for translucent colours.
        if color.a() < 250 {
            p.circle_filled(rect.center(), r, Color32::WHITE);
        }
        p.circle_filled(rect.center(), r, color);
        p.circle_stroke(rect.center(), r, EStroke::new(1.0, pal.border));
    }
    resp
}

fn vsep(ui: &mut egui::Ui, pal: &Palette) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(9.0, 26.0), Sense::hover());
    ui.painter().line_segment(
        [rect.center_top(), rect.center_bottom()],
        EStroke::new(1.0, pal.border),
    );
}

fn floating_frame(pal: &Palette) -> Frame {
    Frame::new()
        .fill(pal.surface)
        .stroke(EStroke::new(1.0, pal.border))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::symmetric(8, 6))
        .shadow(Shadow {
            offset: [0, 4],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(36),
        })
}

fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> egui::Response {
    ui.add(
        egui::Button::new(label)
            .shortcut_text(shortcut)
            .frame_when_inactive(false)
            .min_size(Vec2::new(250.0, 30.0)),
    )
}

impl EditorApp {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, "settings"))
            .unwrap_or_default();
        theme::apply(&cc.egui_ctx, settings.dark);
        let mut app = Self {
            note: Note::new(),
            path: None,
            opened_not_saved: false,
            modified: false,
            settings,
            tool: Tool::Pen,
            zoom: 0.6,
            fit_pending: true,
            preview: false,
            selection: None,
            editing_text: None,
            current_page: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            text_edit_snapshot: None,
            drawing: None,
            dragging: None,
            erasing: false,
            worker: Worker::new(cc.egui_ctx.clone()),
            textures: HashMap::new(),
            thumbs: HashMap::new(),
            page_versions: HashMap::new(),
            erase_masks: HashMap::new(),
            text_version: 0,
            version_counter: 0,
            image_cache: HashMap::new(),
            status: String::new(),
            shown_status: String::new(),
            status_time: 0.0,
            errors: Vec::new(),
            show_info: false,
            show_about: false,
            show_pages: false,
            show_body: false,
            tool_popup: false,
            selection_anchor: None,
            pending: None,
            scroll_to_page: None,
            focus_text: false,
        };
        if let Some(p) = open {
            app.open_path(p);
        }
        app
    }

    // ---------- document state ----------

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            pages: self.note.pages.clone(),
            title: self.note.title.clone(),
            body: self.note.body.clone(),
        }
    }

    fn push_undo(&mut self) {
        let s = self.snapshot();
        self.push_undo_snapshot(s);
    }

    fn push_undo_snapshot(&mut self, s: Snapshot) {
        self.undo.push(s);
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.modified = true;
    }

    fn restore(&mut self, s: Snapshot) {
        let text_changed = s.title != self.note.title || s.body != self.note.body;
        let structure_changed = s.pages.len() != self.note.pages.len()
            || s.pages
                .iter()
                .zip(&self.note.pages)
                .any(|(a, b)| a.id != b.id);
        let changed: Vec<String> = s.pages.iter().map(|p| p.id.clone()).collect();
        self.note.pages = s.pages;
        self.note.title = s.title;
        self.note.body = s.body;
        self.selection = None;
        self.editing_text = None;
        self.text_edit_snapshot = None;
        self.erase_masks.clear();
        for id in changed {
            self.bump_page(&id);
        }
        if text_changed || structure_changed {
            self.text_version += 1;
        }
        self.current_page = self.current_page.min(self.note.pages.len() - 1);
        self.modified = true;
    }

    fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            let cur = self.snapshot();
            self.redo.push(cur);
            self.restore(s);
        }
    }

    fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            let cur = self.snapshot();
            self.undo.push(cur);
            self.restore(s);
        }
    }

    fn bump_page(&mut self, id: &str) {
        self.version_counter += 1;
        self.page_versions
            .insert(id.to_owned(), self.version_counter);
    }

    fn page_changed(&mut self, page: usize) {
        self.note.pages[page].dirty = true;
        let id = self.note.pages[page].id.clone();
        self.bump_page(&id);
        self.modified = true;
    }

    fn reset_document(&mut self, note: Note, path: Option<PathBuf>) {
        self.note = note;
        self.path = path;
        self.modified = false;
        self.undo.clear();
        self.redo.clear();
        self.selection = None;
        self.editing_text = None;
        self.text_edit_snapshot = None;
        self.current_page = 0;
        self.fit_pending = true;
        self.textures.clear();
        self.thumbs.clear();
        self.image_cache.clear();
        self.page_versions.clear();
        self.erase_masks.clear();
        self.text_version += 1;
        self.scroll_to_page = Some(0);
    }

    fn open_path(&mut self, path: PathBuf) {
        match model::open(&path) {
            Ok(note) => {
                self.status = format!(
                    "{} açıldı ({} sayfa)",
                    path.file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    note.pages.len()
                );
                self.show_info = !note.warnings.is_empty();
                self.settings.last_dir = path.parent().map(PathBuf::from);
                self.reset_document(note, Some(path));
                self.opened_not_saved = true;
            }
            Err(e) => self.errors.push(format!("{}: {e}", path.display())),
        }
    }

    fn open_dialog(&mut self) {
        let mut d = rfd::FileDialog::new().add_filter("Samsung Notes", &["sdocx"]);
        if let Some(dir) = &self.settings.last_dir {
            d = d.set_directory(dir);
        }
        if let Some(p) = d.pick_file() {
            self.open_path(p);
        }
    }

    fn suggested_name(&self) -> String {
        let stem = self
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                if self.note.title.trim().is_empty() {
                    "Not".into()
                } else {
                    self.note.title.trim().into()
                }
            });
        if self.opened_not_saved {
            format!("{stem} (düzenlendi).sdocx")
        } else {
            format!("{stem}.sdocx")
        }
    }

    /// Returns true when the note was saved.
    fn save(&mut self, save_as: bool) -> bool {
        self.finish_text_edit();
        let target = match (&self.path, save_as || self.opened_not_saved) {
            (Some(p), false) => Some(p.clone()),
            _ => {
                let mut d = rfd::FileDialog::new()
                    .add_filter("Samsung Notes", &["sdocx"])
                    .set_file_name(self.suggested_name());
                if let Some(dir) = self
                    .path
                    .as_ref()
                    .and_then(|p| p.parent())
                    .or(self.settings.last_dir.as_deref())
                {
                    d = d.set_directory(dir);
                }
                d.save_file().map(|p| {
                    if p.extension().is_none() {
                        p.with_extension("sdocx")
                    } else {
                        p
                    }
                })
            }
        };
        let Some(target) = target else { return false };
        if self.opened_not_saved && self.path.as_ref() == Some(&target) {
            // First overwrite of a file opened from disk: keep a backup copy.
            let backup = target.with_extension("sdocx.yedek");
            if !backup.exists() {
                let _ = std::fs::copy(&target, &backup);
            }
        }
        match model::save(&self.note, &target) {
            Ok(report) => {
                // Reload what was written so the view shows the saved file exactly.
                match model::open(&target) {
                    Ok(mut reloaded) => {
                        reloaded.warnings.clear();
                        let undo = std::mem::take(&mut self.undo);
                        let redo = std::mem::take(&mut self.redo);
                        let page = self.current_page;
                        self.reset_document(reloaded, Some(target.clone()));
                        self.undo = undo;
                        self.redo = redo;
                        self.current_page = page.min(self.note.pages.len() - 1);
                        self.scroll_to_page = None;
                        self.fit_pending = false;
                    }
                    Err(e) => self
                        .errors
                        .push(format!("Kaydedilen dosya yeniden açılamadı: {e}")),
                }
                self.opened_not_saved = false;
                self.modified = false;
                self.settings.last_dir = target.parent().map(PathBuf::from);
                let _ = report.pages_copied;
                self.status = format!(
                    "Kaydedildi: {}",
                    target
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                );
                if report.pages_rebuilt + report.pages_new > 0 {
                    self.status.push_str(&format!(
                        " · {} sayfa güncellendi",
                        report.pages_rebuilt + report.pages_new
                    ));
                }
                for n in report.notes {
                    self.status.push_str(" · ");
                    self.status.push_str(&n);
                }
                true
            }
            Err(e) => {
                self.errors.push(format!("Kaydedilemedi: {e}"));
                false
            }
        }
    }

    fn export_pdf(&mut self) {
        let name = self
            .suggested_name()
            .replace(".sdocx", ".pdf")
            .replace(" (düzenlendi)", "");
        let mut d = rfd::FileDialog::new()
            .add_filter("PDF", &["pdf"])
            .set_file_name(name);
        if let Some(dir) = self
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .or(self.settings.last_dir.as_deref())
        {
            d = d.set_directory(dir);
        }
        let Some(target) = d.save_file() else { return };
        match model::export_pdf(&self.note)
            .and_then(|b| std::fs::write(&target, b).map_err(Into::into))
        {
            Ok(()) => self.status = format!("PDF kaydedildi: {}", target.display()),
            Err(e) => self.errors.push(format!("PDF oluşturulamadı: {e}")),
        }
    }

    fn insert_image(&mut self) {
        let mut d = rfd::FileDialog::new().add_filter("Resim", IMAGE_EXTS);
        if let Some(dir) = &self.settings.last_dir {
            d = d.set_directory(dir);
        }
        if let Some(path) = d.pick_file() {
            self.insert_image_path(&path, None);
        }
    }

    /// Adds an image to the current page (centred, or at `at` in page units).
    fn insert_image_path(&mut self, path: &Path, at: Option<(usize, [f32; 2])>) {
        self.finish_text_edit();
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                self.errors.push(format!("Resim okunamadı: {e}"));
                return;
            }
        };
        let (w, h) = match image::load_from_memory(&data) {
            Ok(img) => (img.width() as f32, img.height() as f32),
            Err(e) => {
                self.errors.push(format!("Resim çözülemedi: {e}"));
                return;
            }
        };
        let pi = at
            .map_or(self.current_page, |a| a.0)
            .min(self.note.pages.len() - 1);
        let page = &self.note.pages[pi];
        let max_w = page.width as f32 * 0.6;
        let scale = (max_w / w).min(page.height as f32 * 0.5 / h).min(1.0);
        let (iw, ih) = (w * scale, h * scale);
        let (left, top) = match at {
            Some((_, p)) => (
                (p[0] - iw / 2.0).clamp(0.0, (page.width as f32 - iw).max(0.0)),
                (p[1] - ih / 2.0).clamp(0.0, (page.height as f32 - ih).max(0.0)),
            ),
            None => ((page.width as f32 - iw) / 2.0, page.height as f32 * 0.15),
        };
        self.push_undo();
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "resim.png".into());
        self.note.pages[pi]
            .elements
            .push(Element::Image(model::Image {
                rect: model::Rect::new(left, top, left + iw, top + ih),
                data: Arc::new(data),
                name,
                raw: None,
                source_bind_id: None,
            }));
        self.selection = Some((pi, self.note.pages[pi].elements.len() - 1));
        self.tool = Tool::Select;
        self.current_page = pi;
        self.page_changed(pi);
        self.status = "Resim eklendi".into();
    }

    fn add_page(&mut self) {
        self.finish_text_edit();
        let cur = &self.note.pages[self.current_page];
        let page = Page::blank(cur.width, cur.height, cur.background);
        self.push_undo();
        let at = self.current_page + 1;
        self.note.pages.insert(at, page);
        self.current_page = at;
        self.selection = None;
        self.text_version += 1;
        self.scroll_to_page = Some(at);
    }

    fn delete_page(&mut self) {
        if self.note.pages.len() <= 1 {
            return;
        }
        self.finish_text_edit();
        self.push_undo();
        self.note.pages.remove(self.current_page);
        self.current_page = self.current_page.min(self.note.pages.len() - 1);
        self.selection = None;
        self.text_version += 1;
    }

    fn delete_selection(&mut self) {
        self.editing_text = None;
        self.text_edit_snapshot = None;
        if let Some((p, i)) = self.selection.take()
            && p < self.note.pages.len()
            && i < self.note.pages[p].elements.len()
        {
            self.push_undo();
            self.note.pages[p].elements.remove(i);
            self.page_changed(p);
        }
    }

    fn request_action(&mut self, action: Pending) {
        if self.modified {
            self.pending = Some(action);
        } else {
            self.run_pending(action);
        }
    }

    fn run_pending(&mut self, action: Pending) {
        match action {
            Pending::New => {
                self.reset_document(Note::new(), None);
                self.opened_not_saved = false;
                self.status = "Yeni not".into();
            }
            Pending::Open(Some(p)) => self.open_path(p),
            Pending::Open(None) => self.open_dialog(),
            Pending::Quit => {}
        }
    }

    // ---------- rendering ----------

    fn tex_key(&self, page: &Page, scale_q: u32) -> TexKey {
        TexKey {
            page_version: self.page_versions.get(&page.id).copied().unwrap_or(0),
            text_version: self.text_version,
            mode_full: self.preview,
            scale_q,
        }
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        while let Ok(out) = self.worker.rx.try_recv() {
            match out {
                Output::Page { id, key, image } => {
                    let color = egui::ColorImage::from_rgba_unmultiplied(
                        [image.width as usize, image.height as usize],
                        &image.pixels,
                    );
                    let tex =
                        ctx.load_texture(format!("page-{id}"), color, egui::TextureOptions::LINEAR);
                    if let Some(m) = self.erase_masks.get_mut(&id) {
                        m.retain(|(v, _)| *v > key.page_version);
                        if m.is_empty() {
                            self.erase_masks.remove(&id);
                        }
                    }
                    if key.scale_q == 0 {
                        self.thumbs.insert(id, PageTex { tex, key });
                        continue;
                    }
                    self.textures.insert(id, PageTex { tex, key });
                }
                Output::Error(e) => {
                    if !self.errors.contains(&e) {
                        self.errors.push(e);
                    }
                }
                Output::Done => self.worker.busy = false,
            }
        }
    }

    fn request_renders(&mut self, ctx: &egui::Context, visible: &[usize]) {
        if self.worker.busy {
            return;
        }
        let ppp = ctx.pixels_per_point();
        let scale = (self.zoom * ppp).clamp(0.25, 2.0);
        let scale_q = (scale * 4.0).round() as u32;
        let scale = scale_q as f32 / 4.0;
        let mut pages = Vec::new();
        for &i in visible {
            let page = &self.note.pages[i];
            let key = self.tex_key(page, scale_q);
            if self.textures.get(&page.id).is_none_or(|t| t.key != key) {
                pages.push((i, page.id.clone(), key));
            }
        }
        // Drop textures of pages far from the view to bound memory.
        if self.textures.len() > 24 {
            let keep: HashSet<&String> = visible.iter().map(|&i| &self.note.pages[i].id).collect();
            self.textures.retain(|id, _| keep.contains(id));
        }
        if pages.is_empty() {
            self.request_thumbnails(ctx);
            return;
        }
        let max_side = ctx.input(|i| i.max_texture_side).min(8192) as u32;
        self.worker.submit(Job {
            note: Arc::new(self.note.clone()),
            pages,
            mode: if self.preview {
                RenderMode::Full
            } else {
                RenderMode::Underlay
            },
            scale,
            max_side,
        });
    }

    /// Renders small page previews for the page drawer when nothing else is pending.
    fn request_thumbnails(&mut self, ctx: &egui::Context) {
        if !self.show_pages || self.preview {
            return;
        }
        let max_w = self
            .note
            .pages
            .iter()
            .map(|p| p.width)
            .max()
            .unwrap_or(1080) as f32;
        let scale = (300.0 * ctx.pixels_per_point().min(2.0) / 2.0 / max_w).clamp(0.05, 0.5);
        let pages: Vec<(usize, String, TexKey)> = self
            .note
            .pages
            .iter()
            .enumerate()
            .filter_map(|(i, page)| {
                let key = TexKey {
                    scale_q: 0,
                    mode_full: false,
                    text_version: 0,
                    ..self.tex_key(page, 0)
                };
                self.thumbs
                    .get(&page.id)
                    .is_none_or(|t| t.key != key)
                    .then(|| (i, page.id.clone(), key))
            })
            .take(3)
            .collect();
        if pages.is_empty() {
            return;
        }
        self.worker.submit(Job {
            note: Arc::new(self.note.clone()),
            pages,
            mode: RenderMode::Full,
            scale,
            max_side: 1024,
        });
    }

    fn image_texture(
        &mut self,
        ctx: &egui::Context,
        img: &model::Image,
    ) -> Option<egui::TextureHandle> {
        let key = Arc::as_ptr(&img.data) as usize;
        if let Some(t) = self.image_cache.get(&key) {
            return Some(t.clone());
        }
        let decoded = image::load_from_memory(&img.data).ok()?.to_rgba8();
        let size = [decoded.width() as usize, decoded.height() as usize];
        let tex = ctx.load_texture(
            format!("img-{key}"),
            egui::ColorImage::from_rgba_unmultiplied(size, decoded.as_raw()),
            egui::TextureOptions::LINEAR,
        );
        self.image_cache.insert(key, tex.clone());
        Some(tex)
    }

    /// Ends in-place text editing; an empty new text box is dropped.
    fn finish_text_edit(&mut self) {
        let Some((pi, ei)) = self.editing_text.take() else {
            return;
        };
        self.text_edit_snapshot = None;
        let empty = matches!(
            self.note.pages.get(pi).and_then(|p| p.elements.get(ei)),
            Some(Element::Text(t)) if t.text.trim().is_empty() && t.raw.is_none()
        );
        if empty {
            self.note.pages[pi].elements.remove(ei);
            if self.selection == Some((pi, ei)) {
                self.selection = None;
            }
            self.page_changed(pi);
        }
    }

    fn start_text_edit(&mut self, pi: usize, ei: usize) {
        self.finish_text_edit();
        self.selection = Some((pi, ei));
        self.editing_text = Some((pi, ei));
        self.text_edit_snapshot = None;
        self.focus_text = true;
    }

    fn set_tool(&mut self, t: Tool) {
        if self.tool != t {
            self.finish_text_edit();
            self.tool = t;
            self.tool_popup = false;
            if t != Tool::Select {
                self.selection = None;
            }
        }
    }

    fn clear_page_drawings(&mut self) {
        let pi = self.current_page;
        if !self.note.pages[pi]
            .elements
            .iter()
            .any(|e| matches!(e, Element::Stroke(_)))
        {
            return;
        }
        self.push_undo();
        self.note.pages[pi]
            .elements
            .retain(|e| !matches!(e, Element::Stroke(_)));
        self.selection = None;
        self.page_changed(pi);
        self.status = format!(
            "Sayfa {} temizlendi (Ctrl+Z ile geri alabilirsiniz)",
            pi + 1
        );
    }

    // ---------- UI ----------

    fn pal(&self) -> Palette {
        theme::palette(self.settings.dark)
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let sc = |m: Modifiers, k: Key| KeyboardShortcut::new(m, k);
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::S))) {
            self.save(false);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::S)))
        {
            self.save(true);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::O))) {
            self.request_action(Pending::Open(None));
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::N))) {
            self.request_action(Pending::New);
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.editing_text.is_some() {
                self.finish_text_edit();
            } else {
                self.selection = None;
                self.tool_popup = false;
            }
        }
        if typing {
            return;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)))
            || ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Y)))
        {
            self.redo();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Z))) {
            self.undo();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Plus)))
            || ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Equals)))
        {
            self.zoom = (self.zoom * 1.2).min(4.0);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Minus))) {
            self.zoom = (self.zoom / 1.2).max(0.1);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc(Modifiers::COMMAND, Key::Num0))) {
            self.fit_pending = true;
        }
        if ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace)) {
            self.delete_selection();
        }
        let keys = [
            (Key::V, Tool::Select),
            (Key::P, Tool::Pen),
            (Key::H, Tool::Highlighter),
            (Key::E, Tool::Eraser),
            (Key::T, Tool::Text),
        ];
        for (k, t) in keys {
            if ctx.input(|i| i.key_pressed(k) && !i.modifiers.command && !i.modifiers.alt) {
                self.set_tool(t);
            }
        }
    }

    /// Title row: pages drawer, editable title, undo/redo, note text, save and the menu.
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal();
        let w = ui.available_width();
        ui.allocate_ui_with_layout(
            Vec2::new(w, 50.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.add_space(4.0);
                if icon_button(ui, &pal, Icon::Pages, self.show_pages, "Sayfalar", None).clicked() {
                    self.show_pages = !self.show_pages;
                }
                ui.add_space(4.0);
                let before = self.snapshot_if_needed();
                let title_w = (ui.available_width() - 330.0).clamp(120.0, 520.0);
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.note.title)
                        .hint_text("Başlık")
                        .font(FontId::proportional(20.0))
                        .frame(Frame::NONE)
                        .desired_width(title_w),
                );
                if r.changed() {
                    if let Some(s) = before {
                        self.push_undo_snapshot(s);
                    }
                    self.text_version += 1;
                    self.modified = true;
                }
                if r.lost_focus() {
                    self.text_edit_snapshot = None;
                }
                if self.modified {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, pal.accent);
                    ui.label(egui::RichText::new("Kaydedilmedi").small().color(pal.weak));
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(4.0);
                    let more = icon_button(ui, &pal, Icon::More, false, "Menü", None);
                    egui::Popup::menu(&more)
                        .gap(6.0)
                        .show(|ui| self.main_menu(ui));
                    if icon_button(ui, &pal, Icon::Save, false, "Kaydet (Ctrl+S)", None).clicked() {
                        self.save(false);
                    }
                    if icon_button(ui, &pal, Icon::Notes, self.show_body, "Not metni", None)
                        .clicked()
                    {
                        self.show_body = !self.show_body;
                    }
                    vsep(ui, &pal);
                    ui.add_enabled_ui(!self.redo.is_empty(), |ui| {
                        if icon_button(ui, &pal, Icon::Redo, false, "Yinele (Ctrl+Y)", None)
                            .clicked()
                        {
                            self.redo();
                        }
                    });
                    ui.add_enabled_ui(!self.undo.is_empty(), |ui| {
                        if icon_button(ui, &pal, Icon::Undo, false, "Geri al (Ctrl+Z)", None)
                            .clicked()
                        {
                            self.undo();
                        }
                    });
                    if self.worker.busy {
                        ui.add(egui::Spinner::new().size(16.0).color(pal.weak));
                    }
                });
            },
        );
    }

    fn main_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(260.0);
        if menu_item(ui, "Yeni not", "Ctrl+N").clicked() {
            self.request_action(Pending::New);
        }
        if menu_item(ui, "Aç…", "Ctrl+O").clicked() {
            self.request_action(Pending::Open(None));
        }
        ui.separator();
        if menu_item(ui, "Kaydet", "Ctrl+S").clicked() {
            self.save(false);
        }
        if menu_item(ui, "Farklı kaydet…", "Ctrl+Shift+S").clicked() {
            self.save(true);
        }
        if menu_item(ui, "PDF olarak dışa aktar…", "").clicked() {
            self.export_pdf();
        }
        ui.separator();
        if menu_item(ui, "Resim ekle…", "").clicked() {
            self.insert_image();
        }
        if menu_item(ui, "Bu sayfadan sonra sayfa ekle", "").clicked() {
            self.add_page();
        }
        if ui
            .add_enabled_ui(self.note.pages.len() > 1, |ui| {
                menu_item(ui, "Bu sayfayı sil", "")
            })
            .inner
            .clicked()
        {
            self.delete_page();
        }
        ui.separator();
        let mut preview = self.preview;
        if ui
            .checkbox(&mut preview, "Kaydedilecek hâliyle önizle")
            .clicked()
        {
            self.finish_text_edit();
            self.preview = preview;
        }
        let mut dark = self.settings.dark;
        if ui.checkbox(&mut dark, "Koyu tema").clicked() {
            self.settings.dark = dark;
            theme::apply(ui.ctx(), dark);
        }
        ui.separator();
        if menu_item(ui, "Bu not hakkında", "").clicked() {
            self.show_info = true;
        }
        if menu_item(ui, "Hakkında ve sınırlamalar", "").clicked() {
            self.show_about = true;
        }
        ui.separator();
        if menu_item(ui, "Çıkış", "").clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Centred tool row with quick colours, like a tablet note app.
    fn tool_row(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal();
        let w = ui.available_width();
        ui.allocate_ui_with_layout(
            Vec2::new(w, 48.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let quick = matches!(self.tool, Tool::Pen | Tool::Highlighter | Tool::Text);
                // Width is the same for every tool so the buttons never move.
                let est = 6.0 * 42.0 + 20.0 + 6.0 * 34.0 + 20.0;
                ui.add_space(((ui.available_width() - est) / 2.0).max(8.0));
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.add_enabled_ui(!self.preview, |ui| {
                    let tools = [
                        (Tool::Select, Icon::Select, "Seç ve taşı (V)", None),
                        (
                            Tool::Pen,
                            Icon::Pen,
                            "Kalem (P) · ayarlar için tekrar tıklayın",
                            Some(c32(self.settings.pen_color)),
                        ),
                        (
                            Tool::Highlighter,
                            Icon::Highlighter,
                            "Vurgulayıcı (H) · ayarlar için tekrar tıklayın",
                            Some(c32([
                                self.settings.highlighter_color[0],
                                self.settings.highlighter_color[1],
                                self.settings.highlighter_color[2],
                                255,
                            ])),
                        ),
                        (
                            Tool::Eraser,
                            Icon::Eraser,
                            "Silgi (E) · ayarlar için tekrar tıklayın",
                            None,
                        ),
                        (Tool::Text, Icon::Text, "Yazı (T)", None),
                    ];
                    let mut active_resp = None;
                    for (t, icon, tip, tint) in tools {
                        let resp = icon_button(ui, &pal, icon, self.tool == t, tip, tint);
                        if resp.clicked() {
                            if self.tool == t {
                                self.tool_popup = !self.tool_popup;
                            } else {
                                self.set_tool(t);
                            }
                        }
                        if self.tool == t {
                            active_resp = Some(resp);
                        }
                    }
                    if icon_button(ui, &pal, Icon::Image, false, "Resim ekle", None).clicked() {
                        self.insert_image();
                    }
                    if quick {
                        vsep(ui, &pal);
                        let (recent, current) = match self.tool {
                            Tool::Highlighter => (
                                self.settings.highlight_recent.clone(),
                                self.settings.highlighter_color,
                            ),
                            Tool::Text => {
                                (self.settings.pen_recent.clone(), self.settings.text_color)
                            }
                            _ => (self.settings.pen_recent.clone(), self.settings.pen_color),
                        };
                        for c in recent {
                            let shown = if self.tool == Tool::Highlighter {
                                c32([c[0], c[1], c[2], 255])
                            } else {
                                c32(c)
                            };
                            if swatch(ui, &pal, shown, c == current, 30.0).clicked() {
                                self.pick_color(c, false);
                            }
                        }
                        let pr = icon_button_sized(
                            ui,
                            &pal,
                            Icon::Palette,
                            false,
                            "Tüm renkler",
                            None,
                            34.0,
                        );
                        if pr.clicked() {
                            self.tool_popup = !self.tool_popup;
                        }
                    }
                    if let Some(resp) = active_resp {
                        let mut open = self.tool_popup;
                        egui::Popup::from_response(&resp)
                            .open_bool(&mut open)
                            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                            .gap(8.0)
                            .frame(floating_frame(&pal).inner_margin(Margin::same(16)))
                            .show(|ui| self.tool_options(ui));
                        self.tool_popup = open;
                    }
                });
            },
        );
    }

    /// Applies a colour to the current tool (and to the selected element when asked).
    fn pick_color(&mut self, c: [u8; 4], remember: bool) {
        match self.tool {
            Tool::Highlighter => {
                let a = self.settings.highlighter_color[3];
                let c = [c[0], c[1], c[2], if c[3] == 255 { a } else { c[3] }];
                self.settings.highlighter_color = c;
                if remember {
                    push_recent(&mut self.settings.highlight_recent, c);
                }
            }
            Tool::Text => {
                self.settings.text_color = c;
                if remember {
                    push_recent(&mut self.settings.pen_recent, c);
                }
                if let Some((pi, ei)) = self.editing_text.or(self.selection)
                    && let Some(Element::Text(t)) = self
                        .note
                        .pages
                        .get_mut(pi)
                        .and_then(|p| p.elements.get_mut(ei))
                {
                    t.color = argb(c);
                    t.raw = None;
                    self.page_changed(pi);
                }
            }
            _ => {
                self.settings.pen_color = c;
                if remember {
                    push_recent(&mut self.settings.pen_recent, c);
                }
            }
        }
    }

    fn color_grid(&mut self, ui: &mut egui::Ui, pal: &Palette, current: [u8; 4], highlight: bool) {
        let alpha = if highlight { current[3] } else { 255 };
        let colors: Vec<[u8; 3]> = if highlight {
            HIGHLIGHT_PALETTE
                .iter()
                .chain(PALETTE.iter().skip(6).take(12))
                .copied()
                .collect()
        } else {
            PALETTE.to_vec()
        };
        egui::Grid::new(("palette", highlight))
            .spacing(Vec2::new(4.0, 4.0))
            .show(ui, |ui| {
                for (i, c) in colors.iter().enumerate() {
                    let full = [c[0], c[1], c[2], alpha];
                    let sel = current[..3] == c[..];
                    if swatch(ui, pal, c32([c[0], c[1], c[2], 255]), sel, 32.0).clicked() {
                        self.pick_color(full, true);
                    }
                    if i % 6 == 5 {
                        ui.end_row();
                    }
                }
            });
        ui.horizontal(|ui| {
            let mut c = c32([current[0], current[1], current[2], 255]);
            if ui.color_edit_button_srgba(&mut c).changed() {
                let v = c.to_srgba_unmultiplied();
                self.pick_color([v[0], v[1], v[2], alpha], true);
            }
            ui.label(egui::RichText::new("Özel renk").color(pal.weak));
        });
    }

    fn size_row(ui: &mut egui::Ui, pal: &Palette, value: &mut f32, sizes: &[f32], color: Color32) {
        ui.horizontal(|ui| {
            let max = sizes.last().copied().unwrap_or(1.0);
            for &s in sizes {
                let (rect, resp) = ui.allocate_exact_size(Vec2::splat(38.0), Sense::click());
                let sel = (*value - s).abs() < 0.01;
                if sel {
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(10), pal.accent_soft);
                } else if resp.hovered() {
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(10), pal.surface_alt);
                }
                let r = 2.0 + 12.0 * (s / max).sqrt();
                ui.painter().circle_filled(rect.center(), r, color);
                if resp.on_hover_text(format!("{s}")).clicked() {
                    *value = s;
                }
            }
        });
    }

    fn tool_options(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal();
        ui.set_width(236.0);
        let title = |ui: &mut egui::Ui, t: &str| {
            ui.label(egui::RichText::new(t).strong().size(16.0));
        };
        let section = |ui: &mut egui::Ui, t: &str| {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(t).small().color(pal.weak));
        };
        match self.tool {
            Tool::Pen | Tool::Highlighter => {
                let hl = self.tool == Tool::Highlighter;
                title(ui, if hl { "Vurgulayıcı" } else { "Kalem" });
                // Preview of the current pen.
                let (rect, _) = ui.allocate_exact_size(Vec2::new(236.0, 44.0), Sense::hover());
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(10), Color32::WHITE);
                let (color, width) = self.current_pen_for(self.tool);
                let pts: Vec<Pos2> = (0..=40)
                    .map(|i| {
                        let t = i as f32 / 40.0;
                        Pos2::new(
                            rect.left() + 20.0 + t * (rect.width() - 40.0),
                            rect.center().y + (t * 6.5).sin() * 8.0,
                        )
                    })
                    .collect();
                ui.painter().line(
                    pts,
                    EStroke::new(
                        (width * PEN_DISPLAY_FACTOR * 0.8).clamp(1.0, 30.0),
                        from_argb(color),
                    ),
                );
                section(ui, "Kalınlık");
                let shown = from_argb(color | 0xFF00_0000);
                if hl {
                    Self::size_row(
                        ui,
                        &pal,
                        &mut self.settings.highlighter_width,
                        &HIGHLIGHT_SIZES,
                        shown,
                    );
                    ui.add(
                        egui::Slider::new(&mut self.settings.highlighter_width, 6.0..=120.0)
                            .show_value(false),
                    );
                    section(ui, "Saydamlık");
                    let mut a = self.settings.highlighter_color[3] as f32;
                    if ui
                        .add(egui::Slider::new(&mut a, 30.0..=200.0).show_value(false))
                        .changed()
                    {
                        self.settings.highlighter_color[3] = a as u8;
                    }
                } else {
                    Self::size_row(ui, &pal, &mut self.settings.pen_width, &PEN_SIZES, shown);
                    ui.add(
                        egui::Slider::new(&mut self.settings.pen_width, 0.5..=40.0)
                            .logarithmic(true)
                            .show_value(false),
                    );
                }
                section(ui, "Renk");
                let current = if hl {
                    self.settings.highlighter_color
                } else {
                    self.settings.pen_color
                };
                self.color_grid(ui, &pal, current, hl);
            }
            Tool::Eraser => {
                title(ui, "Silgi");
                section(ui, "Boyut");
                ui.add(
                    egui::Slider::new(&mut self.settings.eraser_radius, 4.0..=80.0)
                        .show_value(false),
                );
                ui.label(
                    egui::RichText::new("Dokunduğu çizgiyi bütün olarak siler.")
                        .small()
                        .color(pal.weak),
                );
                ui.add_space(6.0);
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Bu sayfadaki tüm çizimleri sil").color(pal.danger),
                        )
                        .min_size(Vec2::new(236.0, 32.0)),
                    )
                    .clicked()
                {
                    self.clear_page_drawings();
                }
            }
            Tool::Text => {
                title(ui, "Yazı");
                section(ui, "Yazı boyutu");
                ui.add(egui::Slider::new(&mut self.settings.text_size, 4.0..=40.0).suffix(" pt"));
                section(ui, "Renk");
                let current = self.settings.text_color;
                self.color_grid(ui, &pal, current, false);
            }
            Tool::Select => {
                title(ui, "Seç");
                ui.label(
                    "Bir öğeye dokunup sürükleyin. Yazı kutusunu düzenlemek için çift tıklayın.",
                );
            }
        }
    }

    fn current_pen_for(&self, tool: Tool) -> (u32, f32) {
        match tool {
            Tool::Highlighter => (
                argb(self.settings.highlighter_color),
                self.settings.highlighter_width,
            ),
            _ => (argb(self.settings.pen_color), self.settings.pen_width),
        }
    }

    fn current_pen(&self) -> (u32, f32) {
        self.current_pen_for(self.tool)
    }

    /// Left drawer with page thumbnails.
    fn pages_drawer(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal();
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Sayfalar").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!self.preview, |ui| {
                    if icon_button_sized(ui, &pal, Icon::Plus, false, "Sayfa ekle", None, 30.0)
                        .clicked()
                    {
                        self.add_page();
                    }
                });
            });
        });
        ui.add_space(4.0);
        let mut action: Option<(usize, u8)> = None;
        egui::ScrollArea::vertical()
            .id_salt("thumbs")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let w = (ui.available_width() - 24.0).max(60.0);
                for (pi, page) in self.note.pages.iter().enumerate() {
                    let h = w * page.height as f32 / page.width.max(1) as f32;
                    let (outer, resp) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), h + 30.0),
                        Sense::click(),
                    );
                    let r = Rect::from_min_size(
                        Pos2::new(outer.center().x - w / 2.0, outer.top() + 4.0),
                        Vec2::new(w, h),
                    );
                    let p = ui.painter();
                    let current = pi == self.current_page;
                    p.add(
                        Shadow {
                            offset: [0, 2],
                            blur: 8,
                            spread: 0,
                            color: Color32::from_black_alpha(30),
                        }
                        .as_shape(r, CornerRadius::same(4)),
                    );
                    p.rect_filled(
                        r,
                        CornerRadius::same(4),
                        from_argb(page.background | 0xFF00_0000),
                    );
                    if let Some(t) = self.thumbs.get(&page.id).or(self.textures.get(&page.id)) {
                        p.image(
                            t.tex.id(),
                            r,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    p.rect_stroke(
                        r,
                        CornerRadius::same(4),
                        if current {
                            EStroke::new(2.5, pal.accent)
                        } else if resp.hovered() {
                            EStroke::new(1.5, pal.weak)
                        } else {
                            EStroke::new(1.0, pal.border)
                        },
                        StrokeKind::Outside,
                    );
                    p.text(
                        Pos2::new(outer.center().x, r.bottom() + 13.0),
                        Align2::CENTER_CENTER,
                        format!("{}", pi + 1),
                        FontId::proportional(12.0),
                        if current { pal.accent } else { pal.weak },
                    );
                    if resp.clicked() {
                        action = Some((pi, 0));
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Sonrasına boş sayfa ekle").clicked() {
                            action = Some((pi, 1));
                        }
                        if ui
                            .add_enabled(
                                self.note.pages.len() > 1,
                                egui::Button::new("Sayfayı sil"),
                            )
                            .clicked()
                        {
                            action = Some((pi, 2));
                        }
                    });
                }
            });
        if let Some((pi, a)) = action {
            self.current_page = pi;
            match a {
                0 => self.scroll_to_page = Some(pi),
                1 if !self.preview => self.add_page(),
                2 if !self.preview => self.delete_page(),
                _ => {}
            }
        }
    }

    /// Right drawer: the typed note text.
    fn body_drawer(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal();
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Not metni").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button_sized(ui, &pal, Icon::Close, false, "Kapat", None, 28.0).clicked() {
                    self.show_body = false;
                }
            });
        });
        ui.label(
            egui::RichText::new("Notun klavyeyle yazılan metni (el yazısından ayrı).")
                .small()
                .color(pal.weak),
        );
        if self.note.source.is_some() && self.note.text_changed() {
            ui.label(
                egui::RichText::new(
                    "Metin değişti: kaydederken kalın, renk gibi biçimler sadeleşir.",
                )
                .small()
                .color(pal.danger),
            );
        }
        ui.add_space(4.0);
        let before = self.snapshot_if_needed();
        let r = egui::ScrollArea::vertical()
            .id_salt("body")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_enabled(
                    !self.preview,
                    egui::TextEdit::multiline(&mut self.note.body)
                        .hint_text("Metin yazın…")
                        .desired_width(f32::INFINITY)
                        .desired_rows(24)
                        .frame(
                            Frame::new()
                                .fill(pal.surface_alt)
                                .corner_radius(CornerRadius::same(10))
                                .inner_margin(Margin::same(10)),
                        ),
                )
            })
            .inner;
        if r.changed() {
            if let Some(s) = before {
                self.push_undo_snapshot(s);
            }
            self.text_version += 1;
            self.modified = true;
        }
        if r.lost_focus() {
            self.text_edit_snapshot = None;
        }
    }

    /// Takes a snapshot at the start of a text edit, for a single undo step per edit.
    fn snapshot_if_needed(&mut self) -> Option<Snapshot> {
        if self.text_edit_snapshot.is_none() {
            self.text_edit_snapshot = Some(self.snapshot());
            return self.text_edit_snapshot.clone();
        }
        None
    }

    fn page_offsets(&self) -> Vec<f32> {
        let mut y = 0.0;
        self.note
            .pages
            .iter()
            .map(|p| {
                let o = y;
                y += p.height as f32 + PAGE_GAP;
                o
            })
            .collect()
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let pal = self.pal();
        let max_w = self
            .note
            .pages
            .iter()
            .map(|p| p.width)
            .max()
            .unwrap_or(1080) as f32;
        if self.fit_pending {
            self.fit_pending = false;
            self.zoom = ((ui.available_width() - 96.0) / max_w).clamp(0.1, 1.5);
        }
        // Ctrl + wheel zoom.
        let (ctrl, scroll) = ctx.input(|i| (i.modifiers.command, i.smooth_scroll_delta.y));
        if ctrl && scroll != 0.0 && ui.rect_contains_pointer(ui.max_rect()) {
            self.zoom = (self.zoom * (1.0 + scroll * 0.002)).clamp(0.1, 4.0);
        }
        let zoom = self.zoom;
        let offsets = self.page_offsets();
        let total_h = offsets.last().copied().unwrap_or(0.0)
            + self.note.pages.last().map_or(0.0, |p| p.height as f32)
            + ADD_PAGE_SPACE;
        let content = Vec2::new(max_w * zoom + 96.0, total_h * zoom + 48.0);

        let mut area = egui::ScrollArea::both().auto_shrink([false, false]);
        if let Some(p) = self.scroll_to_page.take() {
            area = area.vertical_scroll_offset(offsets.get(p).copied().unwrap_or(0.0) * zoom);
        }
        if ctrl {
            area = area.scroll_source(egui::scroll_area::ScrollSource {
                mouse_wheel: false,
                ..Default::default()
            });
        }
        self.selection_anchor = None;
        area.show_viewport(ui, |ui, viewport| {
            let (rect, response) =
                ui.allocate_exact_size(content.max(ui.available_size()), Sense::click_and_drag());
            let origin =
                rect.min + Vec2::new(((rect.width() - max_w * zoom) / 2.0).max(48.0), 40.0);
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 0.0, pal.canvas);

            let to_screen = |pi: usize, x: f32, y: f32| -> Pos2 {
                origin + Vec2::new(x * zoom, (offsets[pi] + y) * zoom)
            };
            let view = viewport.translate(rect.min.to_vec2());
            let mut visible = Vec::new();
            for (pi, page) in self.note.pages.iter().enumerate() {
                let r = Rect::from_min_max(
                    to_screen(pi, 0.0, 0.0),
                    to_screen(pi, page.width as f32, page.height as f32),
                );
                if r.intersects(view.expand(400.0)) {
                    visible.push(pi);
                }
            }
            // Page shown in the page indicator: the one under the middle of the view.
            if self.drawing.is_none() && self.dragging.is_none() {
                let mid = view.center().y;
                if let Some(pi) = visible.iter().copied().find(|&pi| {
                    let top = to_screen(pi, 0.0, 0.0).y;
                    let bottom = to_screen(pi, 0.0, self.note.pages[pi].height as f32 + PAGE_GAP).y;
                    mid >= top && mid < bottom
                }) && !response.is_pointer_button_down_on()
                    && self.editing_text.is_none()
                    && ctx.input(|i| i.smooth_scroll_delta.y != 0.0)
                {
                    self.current_page = pi;
                }
            }

            // Pages: shadow, background, rendered underlay.
            for &pi in &visible {
                let page = &self.note.pages[pi];
                let prect = Rect::from_min_max(
                    to_screen(pi, 0.0, 0.0),
                    to_screen(pi, page.width as f32, page.height as f32),
                );
                painter.add(
                    Shadow {
                        offset: [0, 3],
                        blur: 14,
                        spread: 0,
                        color: Color32::from_black_alpha(if self.settings.dark { 110 } else { 28 }),
                    }
                    .as_shape(prect, CornerRadius::same(3)),
                );
                painter.rect_filled(
                    prect,
                    CornerRadius::same(3),
                    from_argb(page.background | 0xFF00_0000),
                );
                if let Some(t) = self.textures.get(&page.id) {
                    painter.image(
                        t.tex.id(),
                        prect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    if let Some(masks) = self.erase_masks.get(&page.id) {
                        let bg = from_argb(page.background | 0xFF00_0000);
                        for (v, s) in masks {
                            if *v <= t.key.page_version {
                                continue;
                            }
                            let pts: Vec<Pos2> =
                                s.points.iter().map(|p| to_screen(pi, p[0], p[1])).collect();
                            let w = s.width * PEN_DISPLAY_FACTOR * zoom + 4.0;
                            if pts.len() == 1 {
                                painter.circle_filled(pts[0], w / 2.0, bg);
                            } else {
                                painter.line(pts, EStroke::new(w, bg));
                            }
                        }
                    }
                } else {
                    painter.text(
                        prect.center_top() + Vec2::new(0.0, 40.0f32.min(prect.height() / 2.0)),
                        Align2::CENTER_CENTER,
                        "Yükleniyor…",
                        FontId::proportional(13.0),
                        Color32::from_gray(160),
                    );
                }
            }
            if !self.preview {
                for &pi in &visible {
                    let n = self.note.pages[pi].elements.len();
                    for ei in 0..n {
                        if self.editing_text == Some((pi, ei)) {
                            continue;
                        }
                        let el = self.note.pages[pi].elements[ei].clone();
                        if el.raw().is_some() {
                            continue; // drawn by the renderer
                        }
                        self.paint_element(&ctx, &painter, &el, &|x, y| to_screen(pi, x, y), zoom);
                    }
                }
                // Stroke being drawn.
                if let Some((pi, pts)) = &self.drawing {
                    let (color, width) = self.current_pen();
                    let line: Vec<Pos2> = pts.iter().map(|p| to_screen(*pi, p[0], p[1])).collect();
                    painter.line(
                        line,
                        EStroke::new(
                            (width * PEN_DISPLAY_FACTOR * zoom).max(0.5),
                            from_argb(color),
                        ),
                    );
                }
                // Selection outline.
                if let Some((pi, ei)) = self.selection
                    && self.editing_text != Some((pi, ei))
                    && let Some(el) = self.note.pages.get(pi).and_then(|p| p.elements.get(ei))
                {
                    let b = el.bounds().expand(4.0 / zoom.max(0.1));
                    let r = Rect::from_min_max(
                        to_screen(pi, b.left, b.top),
                        to_screen(pi, b.right, b.bottom),
                    );
                    painter.rect_stroke(
                        r,
                        CornerRadius::same(4),
                        EStroke::new(1.5, pal.accent),
                        StrokeKind::Outside,
                    );
                    for c in [
                        r.left_top(),
                        r.right_top(),
                        r.left_bottom(),
                        r.right_bottom(),
                    ] {
                        painter.circle_filled(c, 4.5, Color32::WHITE);
                        painter.circle_stroke(c, 4.5, EStroke::new(1.5, pal.accent));
                    }
                    if self.dragging.is_none() {
                        self.selection_anchor = Some(r);
                    }
                }
                // In-place text editing.
                self.text_editor(ui, &to_screen, zoom, &pal);
            }

            // Page numbers.
            for &pi in &visible {
                let p = to_screen(pi, self.note.pages[pi].width as f32, 0.0) + Vec2::new(0.0, -8.0);
                painter.text(
                    p,
                    Align2::RIGHT_BOTTOM,
                    format!("{} / {}", pi + 1, self.note.pages.len()),
                    FontId::proportional(12.0),
                    pal.weak,
                );
            }

            // "Add page" button below the last page.
            let mut over_button = false;
            if !self.preview
                && let Some(last) = self.note.pages.len().checked_sub(1)
            {
                let lp = &self.note.pages[last];
                let c =
                    to_screen(last, lp.width as f32 / 2.0, lp.height as f32) + Vec2::new(0.0, 48.0);
                let br = Rect::from_center_size(c, Vec2::new(170.0, 40.0));
                if view.intersects(br) {
                    let resp = ui.interact(br, Id::NULL.with("add-page-end"), Sense::click());
                    over_button = resp.hovered();
                    painter.rect(
                        br,
                        CornerRadius::same(20),
                        if resp.hovered() {
                            pal.surface
                        } else {
                            pal.surface.gamma_multiply(0.9)
                        },
                        EStroke::new(1.0, pal.border),
                        StrokeKind::Inside,
                    );
                    icons::paint(
                        &painter,
                        Rect::from_center_size(
                            br.left_center() + Vec2::new(26.0, 0.0),
                            Vec2::splat(16.0),
                        ),
                        Icon::Plus,
                        pal.accent,
                        pal.accent,
                    );
                    painter.text(
                        br.center() + Vec2::new(10.0, 0.0),
                        Align2::CENTER_CENTER,
                        "Sayfa ekle",
                        FontId::proportional(14.5),
                        pal.text,
                    );
                    if resp.clicked() {
                        self.current_page = last;
                        self.add_page();
                    }
                }
            }

            let heights: Vec<f32> = self.note.pages.iter().map(|p| p.height as f32).collect();
            let from_screen = |pos: Pos2| -> Option<(usize, [f32; 2])> {
                let local = (pos - origin) / zoom;
                for (pi, &h) in heights.iter().enumerate() {
                    let y = local.y - offsets[pi];
                    if y >= -PAGE_GAP / 2.0 && y <= h + PAGE_GAP / 2.0 {
                        return Some((pi, [local.x, y.clamp(0.0, h)]));
                    }
                }
                None
            };
            // Image files dropped onto a page.
            let dropped: Vec<(PathBuf, Option<Pos2>)> = ctx.input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path())
                    .filter(|p| !p.as_os_str().is_empty())
                    .filter(|p| {
                        p.extension()
                            .is_some_and(|e| IMAGE_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
                    })
                    .map(|p| (p.to_path_buf(), i.pointer.hover_pos()))
                    .collect()
            });
            for (path, pos) in dropped {
                let at = pos.and_then(&from_screen);
                self.insert_image_path(&path, at);
            }
            if !self.preview && !over_button {
                self.handle_pointer(&response, &from_screen);
            }
            let cursor = match self.tool {
                _ if self.preview || over_button => None,
                Tool::Pen | Tool::Highlighter | Tool::Eraser => Some(egui::CursorIcon::Crosshair),
                Tool::Text => Some(egui::CursorIcon::Text),
                Tool::Select => None,
            };
            if let Some(c) = cursor
                && response.hovered()
            {
                ctx.set_cursor_icon(c);
            }
            // Eraser outline.
            if self.tool == Tool::Eraser
                && !self.preview
                && let Some(hp) = response.hover_pos()
            {
                let r = self.eraser_radius_page() * zoom;
                painter.circle_stroke(hp, r.max(3.0), EStroke::new(1.0, pal.weak));
            }
            self.request_renders(&ctx, &visible);
        });
    }

    /// Samsung font units per "point" shown in the UI. Notes without a stored
    /// density are drawn at 1 unit per pixel, so scale sizes to the page width.
    fn text_scale(&self, pi: usize) -> f32 {
        let w = self.note.pages.get(pi).map_or(1080.0, |p| p.width as f32);
        ((w / 360.0) / self.note.density.max(0.1)).max(1.0)
    }

    fn eraser_radius_page(&self) -> f32 {
        self.settings.eraser_radius / self.zoom.max(0.1) * 0.5 + self.settings.eraser_radius * 0.5
    }

    /// Draws the text box being edited as an editable field on the page.
    fn text_editor(
        &mut self,
        ui: &mut egui::Ui,
        to_screen: &dyn Fn(usize, f32, f32) -> Pos2,
        zoom: f32,
        pal: &Palette,
    ) {
        let Some((pi, ei)) = self.editing_text else {
            return;
        };
        let Some(Element::Text(t)) = self.note.pages.get(pi).and_then(|p| p.elements.get(ei))
        else {
            self.editing_text = None;
            return;
        };
        let r = Rect::from_min_max(
            to_screen(pi, t.rect.left, t.rect.top),
            to_screen(pi, t.rect.right, t.rect.bottom),
        );
        let bg = from_argb(self.note.pages[pi].background | 0xFF00_0000);
        let painter = ui.painter();
        painter.rect_filled(r.expand(2.0), CornerRadius::same(4), bg);
        painter.rect_stroke(
            r.expand(2.0),
            CornerRadius::same(4),
            EStroke::new(1.5, pal.accent),
            StrokeKind::Outside,
        );
        let size = (t.font_size * self.note.density * zoom).max(4.0);
        let color = from_argb(t.color | 0xFF00_0000);
        let pad = Vec2::new(8.0 * zoom, 4.0 * zoom);
        let mut text = t.text.clone();
        let before = self.snapshot_if_needed();
        let mut child = ui.new_child(UiBuilder::new().max_rect(r.shrink2(pad)));
        let out = egui::TextEdit::multiline(&mut text)
            .id(Id::NULL.with(("edit-text", pi, ei)))
            .frame(Frame::NONE)
            .font(FontId::proportional(size))
            .text_color(color)
            .hint_text("Yazın…")
            .desired_width(r.width() - pad.x * 2.0)
            .desired_rows(1)
            .show(&mut child);
        if std::mem::take(&mut self.focus_text) {
            out.response.request_focus();
        }
        if out.response.changed() {
            if let Some(s) = before {
                self.push_undo_snapshot(s);
            }
            let need = (out.galley.size().y + pad.y * 2.0 + 4.0) / zoom;
            if let Some(Element::Text(t)) = self.note.pages[pi].elements.get_mut(ei) {
                t.text = text;
                t.raw = None;
                if t.rect.height() < need {
                    t.rect.bottom = t.rect.top + need;
                }
            }
            self.page_changed(pi);
        }
    }

    /// Floating actions above the selected element.
    fn selection_bar(&mut self, ctx: &egui::Context) {
        let (Some(anchor), Some((pi, ei))) = (self.selection_anchor, self.selection) else {
            return;
        };
        if self.editing_text.is_some() || self.tool != Tool::Select || self.preview {
            return;
        }
        let Some(el) = self
            .note
            .pages
            .get(pi)
            .and_then(|p| p.elements.get(ei))
            .cloned()
        else {
            return;
        };
        let pal = self.pal();
        let screen = ctx.content_rect();
        let mut pos = anchor.center_top() - Vec2::new(0.0, 12.0);
        let mut pivot = Align2::CENTER_BOTTOM;
        if pos.y < screen.top() + 140.0 {
            pos = anchor.center_bottom() + Vec2::new(0.0, 12.0);
            pivot = Align2::CENTER_TOP;
        }
        egui::Area::new(Id::NULL.with("selection-bar"))
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(pos)
            .constrain(true)
            .show(ctx, |ui| {
                floating_frame(&pal).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        match &el {
                            Element::Text(t) => {
                                if ui.button("Düzenle").clicked() {
                                    self.start_text_edit(pi, ei);
                                }
                                let k = self.text_scale(pi);
                                if ui.button("A−").on_hover_text("Küçült").clicked() {
                                    self.edit_text_props(pi, ei, Some(-k), None);
                                }
                                ui.label(format!("{:.0}", t.font_size / k));
                                if ui.button("A+").on_hover_text("Büyüt").clicked() {
                                    self.edit_text_props(pi, ei, Some(k), None);
                                }
                                vsep(ui, &pal);
                                for c in self.settings.pen_recent.clone() {
                                    if swatch(ui, &pal, c32(c), argb(c) == t.color, 26.0).clicked() {
                                        self.edit_text_props(pi, ei, None, Some(argb(c)));
                                    }
                                }
                            }
                            Element::Stroke(s) => {
                                for c in self.settings.pen_recent.clone() {
                                    let sel = argb(c) & 0xFFFFFF == s.color & 0xFFFFFF;
                                    if swatch(ui, &pal, c32(c), sel, 26.0).clicked() {
                                        self.push_undo();
                                        if let Some(Element::Stroke(s)) =
                                            self.note.pages[pi].elements.get_mut(ei)
                                        {
                                            s.color = (s.color & 0xFF00_0000) | (argb(c) & 0xFFFFFF);
                                            s.raw = None;
                                        }
                                        self.page_changed(pi);
                                    }
                                }
                            }
                            Element::Image(_) => {
                                for (label, f) in [("Küçült", 0.8f32), ("Büyüt", 1.25)] {
                                    if ui.button(label).clicked() {
                                        self.push_undo();
                                        if let Some(Element::Image(img)) =
                                            self.note.pages[pi].elements.get_mut(ei)
                                        {
                                            let r = img.rect;
                                            img.rect = model::Rect::new(
                                                r.left,
                                                r.top,
                                                r.left + r.width() * f,
                                                r.top + r.height() * f,
                                            );
                                            img.raw = None;
                                        }
                                        self.page_changed(pi);
                                    }
                                }
                            }
                            Element::Opaque(o) => {
                                ui.label(
                                    egui::RichText::new(format!("Korunan öğe ({})", o.kind))
                                        .color(pal.weak),
                                )
                                .on_hover_text(
                                    "Bu öğe olduğu gibi korunur. Silebilirsiniz ama taşıyamaz veya değiştiremezsiniz.",
                                );
                            }
                        }
                        vsep(ui, &pal);
                        if icon_button_sized(ui, &pal, Icon::Trash, false, "Sil (Del)", None, 32.0)
                            .clicked()
                        {
                            self.delete_selection();
                        }
                    });
                });
            });
    }

    fn edit_text_props(
        &mut self,
        pi: usize,
        ei: usize,
        size_step: Option<f32>,
        color: Option<u32>,
    ) {
        self.push_undo();
        if let Some(Element::Text(t)) = self.note.pages[pi].elements.get_mut(ei) {
            if let Some(d) = size_step {
                t.font_size = (t.font_size + d).clamp(1.0, 400.0);
            }
            if let Some(c) = color {
                t.color = c;
            }
            t.raw = None;
        }
        self.page_changed(pi);
    }

    /// Zoom and page indicator in the bottom-right corner, and the status toast.
    fn overlays(&mut self, ctx: &egui::Context, canvas: Rect) {
        let pal = self.pal();
        egui::Area::new(Id::NULL.with("zoom-pill"))
            .order(egui::Order::Middle)
            .pivot(Align2::RIGHT_BOTTOM)
            .fixed_pos(canvas.right_bottom() - Vec2::new(24.0, 18.0))
            .show(ctx, |ui| {
                floating_frame(&pal).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        ui.label(
                            egui::RichText::new(format!(
                                "Sayfa {} / {}",
                                self.current_page + 1,
                                self.note.pages.len()
                            ))
                            .color(pal.weak),
                        );
                        vsep(ui, &pal);
                        if icon_button_sized(
                            ui,
                            &pal,
                            Icon::Minus,
                            false,
                            "Uzaklaştır (Ctrl −)",
                            None,
                            30.0,
                        )
                        .clicked()
                        {
                            self.zoom = (self.zoom / 1.2).max(0.1);
                        }
                        ui.add_sized(
                            Vec2::new(44.0, 30.0),
                            egui::Label::new(format!("%{:.0}", self.zoom * 100.0)),
                        );
                        if icon_button_sized(
                            ui,
                            &pal,
                            Icon::Plus,
                            false,
                            "Yakınlaştır (Ctrl +)",
                            None,
                            30.0,
                        )
                        .clicked()
                        {
                            self.zoom = (self.zoom * 1.2).min(4.0);
                        }
                        if icon_button_sized(
                            ui,
                            &pal,
                            Icon::Fit,
                            false,
                            "Sayfa genişliğine sığdır (Ctrl 0)",
                            None,
                            30.0,
                        )
                        .clicked()
                        {
                            self.fit_pending = true;
                        }
                    });
                });
            });

        let now = ctx.input(|i| i.time);
        if self.status != self.shown_status {
            self.shown_status = self.status.clone();
            self.status_time = now;
        }
        let age = now - self.status_time;
        if !self.status.is_empty() && age < 4.0 {
            let alpha = ((4.0 - age) / 0.4).clamp(0.0, 1.0) as f32;
            egui::Area::new(Id::NULL.with("toast"))
                .order(egui::Order::Foreground)
                .interactable(false)
                .pivot(Align2::CENTER_BOTTOM)
                .fixed_pos(canvas.center_bottom() - Vec2::new(0.0, 76.0))
                .show(ctx, |ui| {
                    Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(
                            40,
                            41,
                            46,
                            (235.0 * alpha) as u8,
                        ))
                        .corner_radius(CornerRadius::same(18))
                        .inner_margin(Margin::symmetric(16, 9))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(&self.status)
                                    .color(Color32::from_white_alpha((255.0 * alpha) as u8)),
                            );
                        });
                });
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn paint_element(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        el: &Element,
        to_screen: &dyn Fn(f32, f32) -> Pos2,
        zoom: f32,
    ) {
        match el {
            Element::Stroke(s) => {
                let pts: Vec<Pos2> = s.points.iter().map(|p| to_screen(p[0], p[1])).collect();
                // The sdocx renderer draws Samsung pen widths at about half their value.
                let stroke = EStroke::new(
                    (s.width * PEN_DISPLAY_FACTOR * zoom).max(0.5),
                    from_argb(s.color),
                );
                if pts.len() == 1 {
                    painter.circle_filled(pts[0], stroke.width / 2.0, stroke.color);
                } else {
                    painter.line(pts, stroke);
                }
            }
            Element::Text(t) => {
                let r = Rect::from_min_max(
                    to_screen(t.rect.left, t.rect.top),
                    to_screen(t.rect.right, t.rect.bottom),
                );
                if self.tool == Tool::Text || self.tool == Tool::Select {
                    painter.rect_stroke(
                        r,
                        CornerRadius::same(3),
                        EStroke::new(0.8, Color32::from_gray(200)),
                        StrokeKind::Inside,
                    );
                }
                let size = (t.font_size * self.note.density * zoom).max(1.0);
                let galley = painter.layout(
                    t.text.clone(),
                    FontId::proportional(size),
                    from_argb(t.color),
                    (r.width() - 16.0 * zoom).max(10.0),
                );
                painter.with_clip_rect(r).galley(
                    r.min + Vec2::new(8.0 * zoom, 4.0 * zoom),
                    galley,
                    from_argb(t.color),
                );
            }
            Element::Image(img) => {
                let r = Rect::from_min_max(
                    to_screen(img.rect.left, img.rect.top),
                    to_screen(img.rect.right, img.rect.bottom),
                );
                match self.image_texture(ctx, img) {
                    Some(tex) => {
                        painter.image(
                            tex.id(),
                            r,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    None => {
                        painter.rect_stroke(
                            r,
                            0.0,
                            EStroke::new(1.0, Color32::RED),
                            StrokeKind::Inside,
                        );
                    }
                }
            }
            Element::Opaque(_) => {}
        }
    }

    fn hit_test(&self, pi: usize, p: [f32; 2]) -> Option<usize> {
        let tol = 6.0 / self.zoom;
        self.note.pages[pi]
            .elements
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, e)| {
                let hit = match e {
                    Element::Stroke(s) => stroke_hit(s, p, tol),
                    Element::Opaque(o) => !o.hidden && o.bounds.contains(p[0], p[1]),
                    other => other.bounds().contains(p[0], p[1]),
                };
                hit.then_some(i)
            })
    }

    fn handle_pointer(
        &mut self,
        response: &egui::Response,
        from_screen: &dyn Fn(Pos2) -> Option<(usize, [f32; 2])>,
    ) {
        let pos = response.interact_pointer_pos().or(response.hover_pos());
        let Some(pos) = pos else { return };
        // A click on the page outside the text being typed ends the edit.
        if self.editing_text.is_some() && (response.clicked() || response.drag_started()) {
            self.finish_text_edit();
            if self.tool == Tool::Text {
                return;
            }
        }
        let Some((pi, p)) = from_screen(pos) else {
            if response.drag_stopped() {
                self.finish_stroke();
                self.dragging = None;
                self.erasing = false;
            }
            if response.clicked() {
                self.selection = None;
            }
            return;
        };

        match self.tool {
            Tool::Pen | Tool::Highlighter => {
                if response.drag_started()
                    || (response.is_pointer_button_down_on() && self.drawing.is_none())
                {
                    self.current_page = pi;
                    self.drawing = Some((pi, vec![p]));
                } else if response.dragged()
                    && let Some((dpi, pts)) = &mut self.drawing
                    && *dpi == pi
                    && pts
                        .last()
                        .is_none_or(|l| (l[0] - p[0]).abs() + (l[1] - p[1]).abs() > 0.6)
                {
                    pts.push(p);
                }
                if response.drag_stopped() || response.clicked() {
                    self.finish_stroke();
                }
            }
            Tool::Eraser => {
                if response.is_pointer_button_down_on() {
                    self.current_page = pi;
                    let r = self.eraser_radius_page();
                    let hits: Vec<usize> = self.note.pages[pi]
                        .elements
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| matches!(e, Element::Stroke(s) if stroke_hit(s, p, r)))
                        .map(|(i, _)| i)
                        .collect();
                    if !hits.is_empty() {
                        if !self.erasing {
                            self.push_undo();
                            self.erasing = true;
                        }
                        let mut erased = Vec::new();
                        for i in hits.into_iter().rev() {
                            if let Element::Stroke(s) = self.note.pages[pi].elements.remove(i)
                                && s.raw.is_some()
                            {
                                erased.push(s);
                            }
                        }
                        self.selection = None;
                        self.page_changed(pi);
                        if !erased.is_empty() {
                            let id = self.note.pages[pi].id.clone();
                            let v = self.page_versions.get(&id).copied().unwrap_or(0);
                            let masks = self.erase_masks.entry(id).or_default();
                            masks.extend(erased.into_iter().map(|s| (v, s)));
                        }
                    }
                }
                if response.drag_stopped() || response.clicked() {
                    self.erasing = false;
                }
            }
            Tool::Select => {
                if response.double_clicked()
                    && let Some(i) = self.hit_test(pi, p)
                    && matches!(self.note.pages[pi].elements[i], Element::Text(_))
                {
                    self.start_text_edit(pi, i);
                    return;
                }
                if response.drag_started() || response.clicked() {
                    self.current_page = pi;
                    let hit = self.hit_test(pi, p);
                    self.selection = hit.map(|i| (pi, i));
                    if response.drag_started()
                        && let Some(i) = hit
                        && self.note.pages[pi].elements[i].is_movable()
                    {
                        self.push_undo();
                        self.dragging = Some((pi, i, pos));
                    }
                } else if response.dragged()
                    && let Some((dpi, i, last)) = self.dragging
                {
                    let d = (pos - last) / self.zoom;
                    if d != Vec2::ZERO {
                        self.note.pages[dpi].elements[i].translate(d.x, d.y);
                        self.dragging = Some((dpi, i, pos));
                        self.page_changed(dpi);
                    }
                }
                if response.drag_stopped() {
                    self.dragging = None;
                }
            }
            Tool::Text => {
                if response.clicked() {
                    self.current_page = pi;
                    if let Some(i) = self
                        .hit_test(pi, p)
                        .filter(|&i| matches!(self.note.pages[pi].elements[i], Element::Text(_)))
                    {
                        self.start_text_edit(pi, i);
                        return;
                    }
                    let page_w = self.note.pages[pi].width as f32;
                    let size = self.settings.text_size * self.text_scale(pi);
                    let w = (page_w * 0.5).min(page_w - p[0]).max(120.0);
                    let h = size * self.note.density * 1.6 + 16.0;
                    self.push_undo();
                    self.note.pages[pi]
                        .elements
                        .push(Element::Text(model::TextBox {
                            rect: model::Rect::new(p[0], p[1], p[0] + w, p[1] + h),
                            text: String::new(),
                            font_size: size,
                            color: argb(self.settings.text_color),
                            raw: None,
                        }));
                    let ei = self.note.pages[pi].elements.len() - 1;
                    self.page_changed(pi);
                    self.start_text_edit(pi, ei);
                }
            }
        }
    }

    fn finish_stroke(&mut self) {
        let Some((pi, pts)) = self.drawing.take() else {
            return;
        };
        if pts.is_empty() {
            return;
        }
        let (color, width) = self.current_pen();
        self.push_undo();
        let n = pts.len();
        self.note.pages[pi]
            .elements
            .push(Element::Stroke(model::Stroke {
                points: pts,
                pressures: vec![0.7; n],
                color,
                width,
                raw: None,
            }));
        self.page_changed(pi);
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(action) = &self.pending {
            let label = match action {
                Pending::Quit => "Çıkmadan önce kaydedilsin mi?",
                _ => "Kaydedilmemiş değişiklikler var. Kaydedilsin mi?",
            };
            let mut choice = None;
            egui::Modal::new(egui::Id::unique("unsaved")).show(ctx, |ui| {
                ui.label(label);
                ui.horizontal(|ui| {
                    if ui.button("Kaydet").clicked() {
                        choice = Some(0);
                    }
                    if ui.button("Kaydetme").clicked() {
                        choice = Some(1);
                    }
                    if ui.button("İptal").clicked() {
                        choice = Some(2);
                    }
                });
            });
            if let Some(c) = choice {
                let action = self.pending.take().unwrap();
                let proceed = match c {
                    0 => self.save(false),
                    1 => true,
                    _ => false,
                };
                if proceed {
                    if matches!(action, Pending::Quit) {
                        self.modified = false;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    } else {
                        self.modified = false;
                        self.run_pending(action);
                    }
                }
            }
        }

        if !self.errors.is_empty() {
            let mut open = true;
            egui::Window::new("Sorun")
                .open(&mut open)
                .collapsible(false)
                .show(ctx, |ui| {
                    for e in &self.errors {
                        ui.label(e);
                    }
                });
            if !open {
                self.errors.clear();
            }
        }

        if self.show_info {
            let mut open = true;
            egui::Window::new("Bu not hakkında").open(&mut open).default_width(460.0).default_pos([360.0, 160.0]).show(ctx, |ui| {
                let counts = self.note.pages.iter().flat_map(|p| &p.elements).fold([0usize; 4], |mut c, e| {
                    c[match e {
                        Element::Stroke(_) => 0,
                        Element::Text(_) => 1,
                        Element::Image(_) => 2,
                        Element::Opaque(_) => 3,
                    }] += 1;
                    c
                });
                ui.label(format!(
                    "{} sayfa · {} çizim · {} yazı kutusu · {} resim · {} düzenlenemeyen öğe",
                    self.note.pages.len(),
                    counts[0],
                    counts[1],
                    counts[2],
                    counts[3]
                ));
                if self.note.pages.iter().any(|p| p.has_special_background) {
                    ui.label("Bazı sayfaların PDF veya şablon arka planı var. Ekranda gösterilmeyebilir ama dosyada korunur.");
                }
                for w in &self.note.warnings {
                    ui.label(format!("• {w}"));
                }
                if !self.note.diagnostics.is_empty() {
                    ui.collapsing(format!("Teknik ayrıntılar ({})", self.note.diagnostics.len()), |ui| {
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            for w in &self.note.diagnostics {
                                ui.small(w);
                            }
                        });
                    });
                }
            });
            if !open {
                self.show_info = false;
            }
        }

        if self.show_about {
            let mut open = true;
            egui::Window::new("Hakkında").open(&mut open).default_width(520.0).default_pos([300.0, 120.0]).show(ctx, |ui| {
                ui.label(format!("sdocx-editor {} · GPL-3.0", env!("CARGO_PKG_VERSION")));
                ui.label("Samsung Notes (.sdocx) dosyalarını açar, düzenler ve kaydeder. Samsung ile bağlantısı yoktur.");
                ui.separator();
                ui.label("Çalışanlar:");
                ui.label("• El yazısı, yazı kutuları, resimler, not metni ve başlık okunur ve aslına yakın çizilir.");
                ui.label("• Yeni çizim, vurgu, yazı kutusu, resim, sayfa ekleme/silme, taşıma, silgi, geri al.");
                ui.label("• Dokunulmayan sayfalar ve nesneler dosyaya bayt bayt aynen geri yazılır.");
                ui.label("• PDF olarak dışa aktarma.");
                ui.separator();
                ui.label("Sınırlamalar:");
                ui.label("• Kilitli (şifreli) notlar açılamaz.");
                ui.label("• Yeni çizimler tek tip kalemle (dolma kalem) kaydedilir; basınç algılanmaz.");
                ui.label("• Şekil, tablo, formül, ses kaydı gibi öğeler korunur ama düzenlenemez.");
                ui.label("• Not metni değiştirilirse biçimlendirmesi (kalın, renk, liste) sadeleşir.");
                ui.label("• PDF arka planlı sayfalarda PDF ekranda görünmeyebilir; dosyada korunur.");
                ui.label("• Kaydedilen dosyalar Samsung'un bütünlük özetleriyle doğrulanır, ancak Samsung Notes'un kendisiyle test edilmemiştir.");
                ui.separator();
                ui.label("Okuma: twangodev/sdocx (GPL-3.0). Yazma: Downhillghost/Dietrich (MIT) yazıcısının Rust uyarlaması.");
            });
            if !open {
                self.show_about = false;
            }
        }
    }
}

impl eframe::App for EditorApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "settings", &self.settings);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let pal = self.pal();
        self.poll_worker(&ctx);

        // .sdocx files dropped onto the window (images are handled by the canvas).
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path())
                .filter(|p| !p.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .collect()
        });
        for p in dropped {
            let ext = p
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if ext == "sdocx" {
                self.request_action(Pending::Open(Some(p)));
                break;
            } else if !IMAGE_EXTS.contains(&ext.as_str()) {
                self.errors
                    .push("Yalnızca .sdocx notları ve resimler bırakılabilir.".into());
            }
        }

        if ctx.input(|i| i.viewport().close_requested()) && self.modified && self.pending.is_none()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Pending::Quit);
        }

        self.handle_shortcuts(&ctx);

        let title = format!(
            "{}{} — sdocx-editor",
            if self.modified { "• " } else { "" },
            self.path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Yeni not".into())
        );
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        let bar = Frame::new()
            .fill(pal.surface)
            .inner_margin(Margin::symmetric(8, 2))
            .stroke(EStroke::NONE);
        egui::Panel::top("title-bar")
            .frame(bar)
            .show_separator_line(false)
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::top("tool-row")
            .frame(bar)
            .show_separator_line(true)
            .show(ui, |ui| self.tool_row(ui));
        let drawer = Frame::new()
            .fill(pal.surface)
            .inner_margin(Margin::symmetric(12, 4));
        if self.show_pages {
            egui::Panel::left("pages")
                .frame(drawer)
                .resizable(true)
                .default_size(190.0)
                .size_range(140.0..=320.0)
                .show(ui, |ui| self.pages_drawer(ui));
        }
        if self.show_body {
            egui::Panel::right("body")
                .frame(drawer)
                .resizable(true)
                .default_size(320.0)
                .size_range(240.0..=600.0)
                .show(ui, |ui| self.body_drawer(ui));
        }
        let canvas_rect = egui::CentralPanel::default()
            .frame(Frame::NONE.fill(pal.canvas))
            .show(ui, |ui| {
                self.canvas(ui);
                ui.max_rect()
            })
            .inner;

        self.selection_bar(&ctx);
        self.overlays(&ctx, canvas_rect);
        self.dialogs(&ctx);
    }
}
