use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, Key, KeyboardShortcut, Modifiers, Pos2, Sense, Stroke as EStroke, StrokeKind,
    Vec2,
};
use sdocx_edit::{self as model, Element, Note, Page, RenderMode};

use crate::worker::{Job, Output, TexKey, Worker};

/// Gap between pages in page units.
const PAGE_GAP: f32 = 40.0;
/// Rendered stroke width relative to the stored pen width (measured with the sdocx renderer).
const PEN_DISPLAY_FACTOR: f32 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Tool {
    Select,
    Pen,
    Highlighter,
    Eraser,
    Text,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub pen_color: [u8; 4],
    pub pen_width: f32,
    pub highlighter_color: [u8; 4],
    pub highlighter_width: f32,
    pub text_size: f32,
    pub text_color: [u8; 4],
    pub eraser_radius: f32,
    pub last_dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            pen_color: [20, 20, 20, 255],
            pen_width: 6.0,
            highlighter_color: [255, 230, 0, 110],
            highlighter_width: 48.0,
            text_size: 10.0,
            text_color: [20, 20, 20, 255],
            eraser_radius: 12.0,
            last_dir: None,
        }
    }
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
    preview: bool,
    selection: Option<(usize, usize)>,
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
    page_versions: HashMap<String, u64>,
    /// Strokes erased from the rendered underlay whose page texture has not been
    /// redrawn yet: painted over in the page colour so erasing feels instant.
    erase_masks: HashMap<String, Vec<(u64, model::Stroke)>>,
    text_version: u64,
    version_counter: u64,
    image_cache: HashMap<usize, egui::TextureHandle>,
    status: String,
    errors: Vec<String>,
    show_info: bool,
    show_about: bool,
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

impl EditorApp {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, "settings"))
            .unwrap_or_default();
        let mut app = Self {
            note: Note::new(),
            path: None,
            opened_not_saved: false,
            modified: false,
            settings,
            tool: Tool::Pen,
            zoom: 0.6,
            preview: false,
            selection: None,
            current_page: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            text_edit_snapshot: None,
            drawing: None,
            dragging: None,
            erasing: false,
            worker: Worker::new(cc.egui_ctx.clone()),
            textures: HashMap::new(),
            page_versions: HashMap::new(),
            erase_masks: HashMap::new(),
            text_version: 0,
            version_counter: 0,
            image_cache: HashMap::new(),
            status: "Yeni not".into(),
            errors: Vec::new(),
            show_info: false,
            show_about: false,
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
        self.current_page = 0;
        self.textures.clear();
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
                    }
                    Err(e) => self
                        .errors
                        .push(format!("Kaydedilen dosya yeniden açılamadı: {e}")),
                }
                self.opened_not_saved = false;
                self.modified = false;
                self.settings.last_dir = target.parent().map(PathBuf::from);
                self.status = format!(
                    "Kaydedildi: {} ({} sayfa aynen korundu, {} sayfa yeniden yazıldı, {} yeni sayfa)",
                    target.display(),
                    report.pages_copied,
                    report.pages_rebuilt,
                    report.pages_new
                );
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
        let mut d = rfd::FileDialog::new()
            .add_filter("Resim", &["png", "jpg", "jpeg", "gif", "webp", "bmp"]);
        if let Some(dir) = &self.settings.last_dir {
            d = d.set_directory(dir);
        }
        let Some(path) = d.pick_file() else { return };
        let data = match std::fs::read(&path) {
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
        let pi = self.current_page;
        let page = &self.note.pages[pi];
        let max_w = page.width as f32 * 0.6;
        let scale = (max_w / w).min(page.height as f32 * 0.5 / h).min(1.0);
        let (iw, ih) = (w * scale, h * scale);
        let left = (page.width as f32 - iw) / 2.0;
        let top = page.height as f32 * 0.15;
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
        self.page_changed(pi);
    }

    fn add_page(&mut self) {
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
        self.push_undo();
        self.note.pages.remove(self.current_page);
        self.current_page = self.current_page.min(self.note.pages.len() - 1);
        self.selection = None;
        self.text_version += 1;
    }

    fn delete_selection(&mut self) {
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

    // ---------- UI ----------

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
        if ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace)) {
            self.delete_selection();
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.selection = None;
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
                self.tool = t;
            }
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("Dosya", |ui| {
                if ui.button("Yeni not  (Ctrl+N)").clicked() {
                    self.request_action(Pending::New);
                }
                if ui.button("Aç…  (Ctrl+O)").clicked() {
                    self.request_action(Pending::Open(None));
                }
                ui.separator();
                if ui.button("Kaydet  (Ctrl+S)").clicked() {
                    self.save(false);
                }
                if ui.button("Farklı kaydet…  (Ctrl+Shift+S)").clicked() {
                    self.save(true);
                }
                if ui.button("PDF olarak dışa aktar…").clicked() {
                    self.export_pdf();
                }
                ui.separator();
                if ui.button("Çıkış").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Düzen", |ui| {
                if ui
                    .add_enabled(
                        !self.undo.is_empty(),
                        egui::Button::new("Geri al  (Ctrl+Z)"),
                    )
                    .clicked()
                {
                    self.undo();
                }
                if ui
                    .add_enabled(!self.redo.is_empty(), egui::Button::new("Yinele  (Ctrl+Y)"))
                    .clicked()
                {
                    self.redo();
                }
                ui.separator();
                if ui
                    .add_enabled(
                        self.selection.is_some(),
                        egui::Button::new("Seçileni sil  (Del)"),
                    )
                    .clicked()
                {
                    self.delete_selection();
                }
                if ui.button("Resim ekle…").clicked() {
                    self.insert_image();
                }
            });
            ui.menu_button("Sayfa", |ui| {
                if ui.button("Bu sayfadan sonra boş sayfa ekle").clicked() {
                    self.add_page();
                }
                if ui
                    .add_enabled(
                        self.note.pages.len() > 1,
                        egui::Button::new("Bu sayfayı sil"),
                    )
                    .clicked()
                {
                    self.delete_page();
                }
            });
            ui.menu_button("Görünüm", |ui| {
                ui.checkbox(
                    &mut self.preview,
                    "Kaydedilecek hâliyle önizle (salt okunur)",
                );
                if ui.button("Yakınlaştır  (Ctrl + tekerlek)").clicked() {
                    self.zoom = (self.zoom * 1.25).min(4.0);
                }
                if ui.button("Uzaklaştır").clicked() {
                    self.zoom = (self.zoom / 1.25).max(0.1);
                }
            });
            ui.menu_button("Yardım", |ui| {
                if ui.button("Bu not hakkında").clicked() {
                    self.show_info = true;
                }
                if ui.button("Hakkında ve sınırlamalar").clicked() {
                    self.show_about = true;
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let tools = [
                (Tool::Select, "Seç (V)"),
                (Tool::Pen, "Kalem (P)"),
                (Tool::Highlighter, "Vurgu (H)"),
                (Tool::Eraser, "Silgi (E)"),
                (Tool::Text, "Yazı (T)"),
            ];
            ui.add_enabled_ui(!self.preview, |ui| {
                for (t, label) in tools {
                    ui.selectable_value(&mut self.tool, t, label);
                }
                if ui.button("Resim ekle").clicked() {
                    self.insert_image();
                }
            });
            ui.separator();
            match self.tool {
                Tool::Pen => {
                    let mut c = c32(self.settings.pen_color);
                    if ui.color_edit_button_srgba(&mut c).changed() {
                        self.settings.pen_color = c.to_srgba_unmultiplied();
                    }
                    for preset in [
                        [20, 20, 20, 255],
                        [33, 82, 214, 255],
                        [214, 40, 40, 255],
                        [30, 150, 70, 255],
                    ] {
                        let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                        ui.painter().circle_filled(r.center(), 7.0, c32(preset));
                        if resp.clicked() {
                            self.settings.pen_color = preset;
                        }
                    }
                    ui.add(
                        egui::Slider::new(&mut self.settings.pen_width, 1.0..=60.0)
                            .text("kalınlık"),
                    );
                }
                Tool::Highlighter => {
                    let mut c = c32(self.settings.highlighter_color);
                    if ui.color_edit_button_srgba(&mut c).changed() {
                        let mut v = c.to_srgba_unmultiplied();
                        v[3] = v[3].min(160);
                        self.settings.highlighter_color = v;
                    }
                    ui.add(
                        egui::Slider::new(&mut self.settings.highlighter_width, 10.0..=120.0)
                            .text("kalınlık"),
                    );
                }
                Tool::Eraser => {
                    ui.add(
                        egui::Slider::new(&mut self.settings.eraser_radius, 4.0..=80.0)
                            .text("silgi boyu"),
                    );
                    ui.label("Dokunduğu çizgileri tümüyle siler.");
                }
                Tool::Text => {
                    let mut c = c32(self.settings.text_color);
                    if ui.color_edit_button_srgba(&mut c).changed() {
                        self.settings.text_color = c.to_srgba_unmultiplied();
                    }
                    ui.add(
                        egui::Slider::new(&mut self.settings.text_size, 4.0..=40.0)
                            .text("yazı boyu"),
                    );
                    ui.label("Sayfaya tıklayın, yazıyı sağdaki panelden girin.");
                }
                Tool::Select => {
                    ui.label("Öğeye tıklayıp sürükleyin; Del ile silin.");
                }
            }
            ui.separator();
            if ui.button(" − ").clicked() {
                self.zoom = (self.zoom / 1.25).max(0.1);
            }
            ui.label(format!("%{:.0}", self.zoom * 100.0));
            if ui.button(" + ").clicked() {
                self.zoom = (self.zoom * 1.25).min(4.0);
            }
            ui.separator();
            ui.toggle_value(&mut self.preview, "Önizleme");
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Not");
        ui.label("Başlık");
        let before = self.snapshot_if_needed();
        let r1 =
            ui.add(egui::TextEdit::singleline(&mut self.note.title).desired_width(f32::INFINITY));
        ui.label("Not metni (klavyeyle yazılan metin)");
        let r2 = egui::ScrollArea::vertical()
            .id_salt("body")
            .max_height(ui.available_height() * 0.45)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.note.body)
                        .desired_width(f32::INFINITY)
                        .desired_rows(8),
                )
            })
            .inner;
        if r1.changed() || r2.changed() {
            if let Some(s) = before {
                self.push_undo_snapshot(s);
            }
            self.text_version += 1;
            self.modified = true;
        }
        if r1.lost_focus() || r2.lost_focus() {
            self.text_edit_snapshot = None;
        }
        if self.note.source.is_some() && self.note.text_changed() {
            ui.colored_label(
                Color32::from_rgb(200, 120, 0),
                "Metin değişti: kaydederken kalın/renk gibi biçimler sadeleşir.",
            );
        }

        ui.separator();
        // Selected element properties.
        let Some((p, i)) = self.selection else {
            ui.label("Bir öğe seçilmedi.");
            return;
        };
        let Some(el) = self.note.pages.get(p).and_then(|pg| pg.elements.get(i)) else {
            self.selection = None;
            return;
        };
        match el {
            Element::Text(_) => {
                ui.heading("Yazı kutusu");
                let before = self.snapshot_if_needed();
                let Some(Element::Text(t)) = self.note.pages[p].elements.get_mut(i) else {
                    return;
                };
                let had_raw = t.raw.is_some();
                let mut changed = false;
                let r = ui.add(
                    egui::TextEdit::multiline(&mut t.text)
                        .hint_text("Yazınızı buraya girin")
                        .desired_width(f32::INFINITY)
                        .desired_rows(5),
                );
                if std::mem::take(&mut self.focus_text) {
                    r.request_focus();
                }
                changed |= r.changed();
                changed |= ui
                    .add(egui::Slider::new(&mut t.font_size, 2.0..=60.0).text("yazı boyu"))
                    .changed();
                let mut c = from_argb(t.color);
                if ui.color_edit_button_srgba(&mut c).changed() {
                    let v = c.to_srgba_unmultiplied();
                    t.color = argb(v);
                    changed = true;
                }
                if had_raw {
                    ui.small("Değiştirirseniz bu kutu sade biçimle yeniden yazılır.");
                }
                if changed {
                    t.raw = None;
                    if let Some(s) = before {
                        self.push_undo_snapshot(s);
                    }
                    self.page_changed(p);
                }
                if r.lost_focus() {
                    self.text_edit_snapshot = None;
                }
            }
            Element::Stroke(s) => {
                ui.heading("Çizim");
                ui.label(format!("{} nokta, kalınlık {:.1}", s.points.len(), s.width));
                let mut c = from_argb(s.color);
                if ui.color_edit_button_srgba(&mut c).changed() {
                    self.push_undo();
                    if let Some(Element::Stroke(s)) = self.note.pages[p].elements.get_mut(i) {
                        s.color = argb(c.to_srgba_unmultiplied());
                        s.raw = None;
                    }
                    self.page_changed(p);
                }
            }
            Element::Image(img) => {
                ui.heading("Resim");
                ui.label(&img.name);
                ui.label(format!(
                    "{:.0} × {:.0}",
                    img.rect.width(),
                    img.rect.height()
                ));
                ui.horizontal(|ui| {
                    let mut factor = None;
                    if ui.button("Küçült").clicked() {
                        factor = Some(0.8);
                    }
                    if ui.button("Büyüt").clicked() {
                        factor = Some(1.25);
                    }
                    if let Some(f) = factor {
                        self.push_undo();
                        if let Some(Element::Image(img)) = self.note.pages[p].elements.get_mut(i) {
                            let r = img.rect;
                            img.rect = model::Rect::new(
                                r.left,
                                r.top,
                                r.left + r.width() * f,
                                r.top + r.height() * f,
                            );
                            img.raw = None;
                        }
                        self.page_changed(p);
                    }
                });
            }
            Element::Opaque(o) => {
                ui.heading("Düzenlenemeyen öğe");
                ui.label(format!("Tür: {}", o.kind));
                ui.label("Bu öğe olduğu gibi korunur. Silebilirsiniz ama taşıyamaz veya değiştiremezsiniz.");
            }
        }
        if ui.button("Seçileni sil").clicked() {
            self.delete_selection();
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
        // Ctrl + wheel zoom.
        let (ctrl, scroll) = ctx.input(|i| (i.modifiers.command, i.smooth_scroll_delta.y));
        if ctrl && scroll != 0.0 && ui.rect_contains_pointer(ui.max_rect()) {
            self.zoom = (self.zoom * (1.0 + scroll * 0.002)).clamp(0.1, 4.0);
        }
        let zoom = self.zoom;
        let offsets = self.page_offsets();
        let max_w = self
            .note
            .pages
            .iter()
            .map(|p| p.width)
            .max()
            .unwrap_or(1080) as f32;
        let total_h = offsets.last().copied().unwrap_or(0.0)
            + self.note.pages.last().map_or(0.0, |p| p.height as f32);
        let content = Vec2::new(max_w * zoom + 64.0, total_h * zoom + 64.0);

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
        area.show_viewport(ui, |ui, viewport| {
            let (rect, response) =
                ui.allocate_exact_size(content.max(ui.available_size()), Sense::click_and_drag());
            let origin =
                rect.min + Vec2::new(((rect.width() - max_w * zoom) / 2.0).max(32.0), 32.0);
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);

            let to_screen = |pi: usize, x: f32, y: f32| -> Pos2 {
                origin + Vec2::new(x * zoom, (offsets[pi] + y) * zoom)
            };
            let view = viewport.translate(rect.min.to_vec2());
            let mut visible = Vec::new();
            for (pi, page) in self.note.pages.iter().enumerate() {
                let r = egui::Rect::from_min_max(
                    to_screen(pi, 0.0, 0.0),
                    to_screen(pi, page.width as f32, page.height as f32),
                );
                if r.intersects(view.expand(400.0)) {
                    visible.push(pi);
                }
            }

            // Pages: background, rendered underlay, then editable elements.
            for &pi in &visible {
                let page = &self.note.pages[pi];
                let prect = egui::Rect::from_min_max(
                    to_screen(pi, 0.0, 0.0),
                    to_screen(pi, page.width as f32, page.height as f32),
                );
                painter.rect_filled(
                    prect.translate(Vec2::splat(3.0)),
                    2.0,
                    Color32::from_black_alpha(40),
                );
                painter.rect_filled(prect, 0.0, from_argb(page.background | 0xFF00_0000));
                if let Some(t) = self.textures.get(&page.id) {
                    painter.image(
                        t.tex.id(),
                        prect,
                        egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
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
                }
                if pi == self.current_page {
                    painter.rect_stroke(
                        prect,
                        0.0,
                        EStroke::new(1.5, Color32::from_rgb(90, 140, 230)),
                        StrokeKind::Outside,
                    );
                }
            }
            if !self.preview {
                for &pi in &visible {
                    let n = self.note.pages[pi].elements.len();
                    for ei in 0..n {
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
                    && let Some(el) = self.note.pages.get(pi).and_then(|p| p.elements.get(ei))
                {
                    let b = el.bounds().expand(4.0);
                    let r = egui::Rect::from_min_max(
                        to_screen(pi, b.left, b.top),
                        to_screen(pi, b.right, b.bottom),
                    );
                    painter.rect_stroke(
                        r,
                        2.0,
                        EStroke::new(1.5, Color32::from_rgb(40, 120, 255)),
                        StrokeKind::Outside,
                    );
                }
            }

            // Page labels.
            for &pi in &visible {
                let p = to_screen(pi, 0.0, 0.0) + Vec2::new(0.0, -4.0);
                painter.text(
                    p,
                    egui::Align2::LEFT_BOTTOM,
                    format!("Sayfa {} / {}", pi + 1, self.note.pages.len()),
                    egui::FontId::proportional(12.0),
                    ui.visuals().weak_text_color(),
                );
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
            if !self.preview {
                self.handle_pointer(&response, &from_screen);
            }
            self.request_renders(&ctx, &visible);
        });
    }

    fn current_pen(&self) -> (u32, f32) {
        match self.tool {
            Tool::Highlighter => (
                argb(self.settings.highlighter_color),
                self.settings.highlighter_width,
            ),
            _ => (argb(self.settings.pen_color), self.settings.pen_width),
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
                let r = egui::Rect::from_min_max(
                    to_screen(t.rect.left, t.rect.top),
                    to_screen(t.rect.right, t.rect.bottom),
                );
                painter.rect_stroke(
                    r,
                    0.0,
                    EStroke::new(0.5, Color32::from_gray(180)),
                    StrokeKind::Inside,
                );
                let size = (t.font_size * self.note.density * zoom).max(1.0);
                let galley = painter.layout(
                    t.text.clone(),
                    egui::FontId::proportional(size),
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
                let r = egui::Rect::from_min_max(
                    to_screen(img.rect.left, img.rect.top),
                    to_screen(img.rect.right, img.rect.bottom),
                );
                match self.image_texture(ctx, img) {
                    Some(tex) => {
                        painter.image(
                            tex.id(),
                            r,
                            egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
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
        let Some((pi, p)) = from_screen(pos) else {
            if response.drag_stopped() {
                self.finish_stroke();
                self.dragging = None;
                self.erasing = false;
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
                    && let Some((_, pts)) = &mut self.drawing
                {
                    let local = p;
                    if pts
                        .last()
                        .is_none_or(|l| (l[0] - local[0]).abs() + (l[1] - local[1]).abs() > 0.6)
                    {
                        pts.push(local);
                    }
                }
                if response.drag_stopped() || response.clicked() {
                    self.finish_stroke();
                }
            }
            Tool::Eraser => {
                if response.is_pointer_button_down_on() {
                    self.current_page = pi;
                    let r = self.settings.eraser_radius / self.zoom.max(0.1) * 0.5
                        + self.settings.eraser_radius * 0.5;
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
                        self.selection = Some((pi, i));
                        return;
                    }
                    let page_w = self.note.pages[pi].width as f32;
                    let size = self.settings.text_size;
                    let w = (page_w * 0.5).min(page_w - p[0]).max(120.0);
                    let h = size * self.note.density * 3.0 + 16.0;
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
                    self.selection = Some((pi, self.note.pages[pi].elements.len() - 1));
                    self.text_edit_snapshot = None;
                    self.focus_text = true;
                    self.page_changed(pi);
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
        self.poll_worker(&ctx);

        // Files dropped onto the window.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if let Some(p) = dropped.into_iter().next() {
            if p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("sdocx"))
            {
                self.request_action(Pending::Open(Some(p)));
            } else {
                self.errors
                    .push("Yalnızca .sdocx dosyaları açılabilir.".into());
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

        egui::Panel::top("menu").show(ui, |ui| {
            self.menu_bar(ui);
            self.toolbar(ui);
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if self.worker.busy {
                    ui.spinner();
                }
                ui.label(&self.status);
            });
        });
        egui::Panel::right("props")
            .resizable(true)
            .default_size(300.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("props-scroll")
                    .show(ui, |ui| self.side_panel(ui));
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.canvas(ui));

        self.dialogs(&ctx);
    }
}
