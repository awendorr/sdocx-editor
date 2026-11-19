//! Opening `.sdocx` files into the editable [`Note`] model.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::Arc;

use sdocx::{PageElement, PageObject, PageObjectContent, StoredObject};

use crate::bin::{identity_hash, rd_str16, rd_u16, rd_u32, rd_u64, sha256};
use crate::model::*;
use crate::Error;

/// Reads every entry of the archive, relative to the folder holding `note.note`.
pub(crate) fn read_entries(bytes: &[u8]) -> Result<Vec<(String, Arc<Vec<u8>>)>, Error> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| Error::Format(format!("ZIP okunamadı: {e}")))?;
    let mut all = Vec::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| Error::Format(format!("ZIP girdisi okunamadı: {e}")))?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().replace('\\', "/");
        let mut data = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut data)?;
        all.push((name, Arc::new(data)));
    }
    let root = all
        .iter()
        .find_map(|(n, _)| n.strip_suffix("note.note").filter(|p| p.is_empty() || p.ends_with('/')).map(str::to_owned))
        .unwrap_or_default();
    Ok(all
        .into_iter()
        .filter_map(|(n, d)| n.strip_prefix(&root).map(|s| (s.to_owned(), d)))
        .collect())
}

fn argb_of(c: Option<sdocx::Color>) -> Argb {
    c.map(|c| 0xFF00_0000 | (c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32).unwrap_or(0xFF00_0000)
}

fn rect_of(b: &sdocx::BoundingBox) -> Rect {
    Rect::new(b.x_min as f32, b.y_min as f32, b.x_max as f32, b.y_max as f32)
}

fn record_end(o: &StoredObject) -> usize {
    match o.children.last() {
        Some(c) => record_end(c),
        None => o.payload_offset + o.payload_size + 32,
    }
}

fn collect_hashes(o: &StoredObject, out: &mut Vec<[u8; 32]>) {
    out.push(o.integrity_trailer);
    for c in &o.children {
        collect_hashes(c, out);
    }
}

fn raw_object(o: &StoredObject, page: &[u8]) -> Option<RawObject> {
    let start = o.payload_offset.checked_sub(7)?;
    let end = record_end(o);
    let bytes = page.get(start..end)?.to_vec();
    let mut hashes = Vec::new();
    collect_hashes(o, &mut hashes);
    Some(RawObject { bytes: Arc::new(bytes), hashes, object_type: page[start] })
}

fn layer_end(l: &sdocx::StoredLayer) -> usize {
    match l.objects.last() {
        Some(o) => record_end(o) + 32,
        None => l.header_offset + l.header_size + 4 + 32,
    }
}

/// Parses the optional layer metadata fields to find the layer identity.
fn layer_identity(page: &[u8], l: &sdocx::StoredLayer) -> Option<(String, u64)> {
    let mut at = l.header_offset + 8;
    let m1 = *page.get(at)? as usize;
    at += 1 + m1;
    let m2len = *page.get(at)? as usize;
    let flags2 = *page.get(at + 1)?;
    at += 1 + m2len + 4;
    if flags2 & 0x01 != 0 {
        at += 4;
    }
    if flags2 & 0x02 != 0 {
        at += 4;
    }
    if flags2 & 0x04 != 0 {
        at = rd_str16(page, at)?.1;
    }
    if flags2 & 0x18 != 0x18 {
        return None;
    }
    let (id, next) = rd_str16(page, at)?;
    let time = rd_u64(page, next)?;
    Some((id, time))
}

fn kind_name(t: sdocx::ObjectType) -> String {
    use sdocx::ObjectType as T;
    match t {
        T::Shape => "şekil".into(),
        T::Line => "çizgi/ok".into(),
        T::Container => "grup".into(),
        T::Voice => "ses kaydı".into(),
        T::Formula | T::Math | T::Plot => "formül".into(),
        T::Table | T::DeprecatedTable => "tablo".into(),
        T::CodeBlock => "kod bloğu".into(),
        T::Web | T::Link => "bağlantı".into(),
        T::Video => "video".into(),
        T::Painting => "boyama".into(),
        T::AttachedFile => "ek dosya".into(),
        T::TextBox => "yazı kutusu".into(),
        T::Image => "resim".into(),
        T::Stroke | T::StrokeBrush | T::StrokeGroup => "çizim".into(),
        other => format!("nesne {}", other.raw()),
    }
}

/// Parses `media/mediaInfo.dat`, keeping each entry body verbatim.
fn parse_media_info(data: &[u8]) -> Option<(u32, Vec<(u32, String, Vec<u8>)>)> {
    let version = rd_u32(data, 0)?;
    let count = rd_u16(data, 4)? as usize;
    let mut at = 6;
    let mut out = Vec::new();
    for _ in 0..count {
        let size = rd_u32(data, at)? as usize;
        let body = data.get(at + 4..at + 4 + size)?.to_vec();
        let bind = rd_u32(&body, 0)?;
        let (name, _) = rd_str16(&body, 4)?;
        out.push((bind, name, body));
        at += 4 + size;
    }
    Some((version, out))
}

pub fn open_bytes(bytes: &[u8]) -> Result<Note, Error> {
    let entries = read_entries(bytes)?;
    let get = |name: &str| entries.iter().find(|(n, _)| n == name).map(|(_, d)| d.clone());

    let options = sdocx::ParseOptions { retain_page_sources: true, ..Default::default() };
    let parsed = match sdocx::parse_bytes_detailed_with_options(bytes, &options) {
        Ok(p) => p,
        Err(e) => {
            let encrypted = get("end_tag.bin")
                .and_then(|d| sdocx::parse_end_tag_bytes(&d).ok())
                .and_then(|t| t.encryption_info().ok().flatten())
                .is_some();
            if encrypted {
                return Err(Error::Encrypted);
            }
            return Err(Error::Format(format!("Not çözümlenemedi: {e}")));
        }
    };
    let encrypted = parsed.end_tag.as_ref().and_then(|t| t.encryption_info().ok().flatten()).is_some();
    if encrypted {
        return Err(Error::Encrypted);
    }

    let mut warnings = Vec::new();
    for d in parsed.report.diagnostics.iter().take(20) {
        warnings.push(format!("{:?}: {}", d.code, d.message));
    }

    let doc = &parsed.document;
    let meta = &doc.metadata;

    // Media files: keep raw entries so existing references stay valid.
    let mut media = Vec::new();
    let mut media_info_version = None;
    if let Some(info) = get("media/mediaInfo.dat")
        && let Some((version, list)) = parse_media_info(&info)
    {
        media_info_version = Some(version);
        for (bind_id, name, body) in list {
            let data = get(&format!("media/{name}")).unwrap_or_default();
            media.push(MediaFile { bind_id, file_name: name, data, raw_entry: Some(body) });
        }
    }
    let media_by_index: HashMap<usize, &sdocx::MediaAsset> = meta.media_assets.iter().enumerate().collect();
    let media_by_id: HashMap<u32, &sdocx::MediaAsset> =
        meta.media_assets.iter().filter_map(|m| m.archive_id.map(|id| (id, m))).collect();

    let manifest_hashes: HashMap<&str, [u8; 32]> = parsed
        .page_manifest
        .iter()
        .flat_map(|m| m.entries.iter().map(|e| (e.page_id.as_str(), e.integrity_hash)))
        .collect();

    let mut pages = Vec::new();
    let mut unsupported: HashMap<String, usize> = HashMap::new();
    for (page, stored) in doc.pages.iter().zip(parsed.stored_pages.iter()) {
        let Some(src) = stored.source_bytes.as_ref().map(|b| Arc::new(b.clone())).or_else(|| get(&stored.archive_entry))
        else {
            warnings.push(format!("Sayfa {} için ham veri bulunamadı.", page.uuid));
            continue;
        };
        let sp = &stored.page;
        let layers = &sp.layers.layers;
        let current = usize::from(sp.layers.current_layer_index);
        let layer_ranges: Vec<(usize, usize)> = layers.iter().map(|l| (l.header_offset, layer_end(l))).collect();
        let page_identity = (sp.header.uuid.clone(), sp.header.modified_time_raw.unwrap_or(0));

        // Check that we know how the stored hashes are computed; otherwise
        // the page gets fresh identities when it is rebuilt.
        let current_layer_identity = layer_identity(&src, &layers[current]).filter(|(id, t)| {
            let mut h = Vec::new();
            for o in &layers[current].objects {
                collect_hashes(o, &mut h);
            }
            let mut buf: Vec<u8> = h.concat();
            buf.extend_from_slice(&identity_hash(id, *t));
            sha256(&buf) == layers[current].integrity_trailer
        });
        let own_hash = src.get(sp.integrity_offset..sp.integrity_offset + 32).and_then(|s| <[u8; 32]>::try_from(s).ok());
        let page_hash = manifest_hashes.get(page.uuid.as_str()).copied().or(own_hash).unwrap_or([0; 32]);

        // Decoded objects by their payload offset.
        let mut decoded: HashMap<usize, &PageObject> = HashMap::new();
        for o in &page.objects {
            if let Some(off) = o.source_offset {
                decoded.entry(off).or_insert(o);
            }
        }

        let mut elements = Vec::new();
        for so in &layers[current].objects {
            let Some(raw) = raw_object(so, &src) else { continue };
            let dec = decoded.get(&so.payload_offset).map(|o| o.content.clone());
            let simple = so.children.is_empty();
            let el = match (&dec, simple) {
                (Some(PageObjectContent::Stroke(s)), true) => Some(Element::Stroke(Stroke {
                    points: s.points.iter().map(|p| [p.x as f32, p.y as f32]).collect(),
                    pressures: if s.pressures.len() == s.points.len() {
                        s.pressures.iter().map(|&p| p as f32).collect()
                    } else {
                        vec![1.0; s.points.len()]
                    },
                    color: argb_of(s.color),
                    width: s.pen_width.max(0.5),
                    raw: Some(raw.clone()),
                })),
                (Some(PageObjectContent::Element(PageElement::TextBox(t))), true) if t.object_spans.is_empty() => {
                    Some(Element::Text(TextBox {
                        rect: rect_of(&t.bbox),
                        text: t.text.clone(),
                        font_size: t.font_size.filter(|f| *f > 0.0).unwrap_or(10.0),
                        color: t.color.map(|c| argb_of(Some(c))).unwrap_or(0xFF00_0000),
                        raw: Some(raw.clone()),
                    }))
                }
                (Some(PageObjectContent::Element(PageElement::PlacedImage(img))), true) => {
                    let asset = img
                        .media_index
                        .and_then(|i| media_by_index.get(&i).copied())
                        .or_else(|| img.media_id.and_then(|id| media_by_id.get(&id).copied()));
                    asset.map(|a| {
                        Element::Image(Image {
                            rect: rect_of(&img.bbox),
                            data: Arc::new(a.data.clone()),
                            name: a.name.clone(),
                            raw: Some(raw.clone()),
                            source_bind_id: a.archive_id,
                        })
                    })
                }
                (Some(PageObjectContent::Element(PageElement::Image { bbox, media_index })), true) => {
                    media_by_index.get(media_index).map(|a| {
                        Element::Image(Image {
                            rect: rect_of(bbox),
                            data: Arc::new(a.data.clone()),
                            name: a.name.clone(),
                            raw: Some(raw.clone()),
                            source_bind_id: a.archive_id,
                        })
                    })
                }
                _ => None,
            };
            let el = el.unwrap_or_else(|| {
                let kind = kind_name(so.object_type);
                *unsupported.entry(kind.clone()).or_default() += 1;
                let bounds = so.base_metadata(&src).map(|m| rect_of(&m.bbox)).unwrap_or_default();
                Element::Opaque(Opaque { bounds, kind, raw: raw.clone(), hidden: dec.is_none() })
            });
            elements.push(el);
        }

        let bg = &page.background;
        let background = bg
            .color_argb
            .or_else(|| page.background_color.map(|c| argb_of(Some(c))))
            .unwrap_or(Note::DEFAULT_BACKGROUND);
        let has_special_background =
            bg.pdf_paper.as_ref().is_some_and(|p| !p.is_empty()) || bg.template_uri.is_some() || bg.image_id.is_some();
        if layers.len() > 1 {
            warnings.push(format!(
                "Bir sayfada {} katman var; yalnızca etkin katman düzenlenebilir, diğerleri olduğu gibi korunur.",
                layers.len()
            ));
        }

        pages.push(Page {
            id: page.uuid.clone(),
            width: page.width,
            height: page.height,
            background,
            elements,
            source: Some(PageSource {
                bytes: src.clone(),
                page_hash,
                layer_offset: layers[0].header_offset.saturating_sub(4),
                layer_count: layers.len(),
                current_layer: current,
                layer_ranges,
                current_layer_identity,
                identity: page_identity,
            }),
            dirty: false,
            has_special_background,
        });
    }

    if !unsupported.is_empty() {
        let mut list: Vec<_> = unsupported.into_iter().map(|(k, n)| format!("{n} {k}")).collect();
        list.sort();
        warnings.push(format!(
            "Düzenlenemeyen öğeler var ({}). Görünürler ve kaydederken olduğu gibi korunurlar.",
            list.join(", ")
        ));
    }

    let title = meta.note_title.as_ref().map(|t| t.text.clone()).unwrap_or_default();
    let body = meta.note_text.as_ref().map(|t| t.text.clone()).unwrap_or_default();
    let source = NoteSource {
        note_bytes: get("note.note"),
        end_tag: get("end_tag.bin"),
        media,
        media_info_version,
        original_title: title.clone(),
        original_body: body.clone(),
        note_dims: parsed.note.as_ref().map(|n| (n.header.width, n.header.height, n.header.page_vertical_padding)),
        encrypted,
        entries,
    };
    if pages.is_empty() {
        pages.push(Page::blank(Note::DEFAULT_WIDTH, Note::DEFAULT_HEIGHT, Note::DEFAULT_BACKGROUND));
    }
    Ok(Note { title, body, pages, density: meta.document_density(), source: Some(Arc::new(source)), warnings })
}

pub fn open(path: impl AsRef<std::path::Path>) -> Result<Note, Error> {
    let bytes = std::fs::read(path)?;
    open_bytes(&bytes)
}
