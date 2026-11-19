//! Writing a [`Note`] back to `.sdocx`.
//!
//! Untouched pages, objects, media and note metadata are copied from the source
//! file byte for byte. Only what was edited is re-encoded (see [`crate::encode`]).

use std::collections::HashMap;
use std::sync::Arc;

use crate::Error;
use crate::bin::{identity_hash, now_us, rd_str16, rd_u16, rd_u32, sha256};
use crate::encode::{self, DEFAULT_NOTE_VERTICAL_PADDING, PAGE_FOOTER};
use crate::model::*;

/// Result of a save.
#[derive(Debug, Default)]
pub struct SaveReport {
    pub pages_copied: usize,
    pub pages_rebuilt: usize,
    pub pages_new: usize,
    pub notes: Vec<String>,
}

struct MediaTable {
    entries: Vec<MediaFile>,
    next_bind: u32,
    by_data: HashMap<usize, u32>,
    now: u64,
}

impl MediaTable {
    fn bind_for(&mut self, img: &Image) -> u32 {
        if let Some(id) = img.source_bind_id
            && self.entries.iter().any(|m| m.bind_id == id)
        {
            return id;
        }
        let key = Arc::as_ptr(&img.data) as usize;
        if let Some(&id) = self.by_data.get(&key) {
            return id;
        }
        let id = self.next_bind;
        self.next_bind += 1;
        let clean: String = img
            .name
            .chars()
            .map(|c| {
                if "<>:\"/\\|?*@".contains(c) || (c as u32) < 32 {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        let clean = if clean.contains('.') {
            clean
        } else {
            format!("{clean}.png")
        };
        let file_name = format!("{id}@{clean}");
        self.entries.push(MediaFile {
            bind_id: id,
            raw_entry: Some(encode::media_entry(id, &file_name, &img.data, self.now)),
            file_name,
            data: img.data.clone(),
        });
        self.by_data.insert(key, id);
        id
    }
}

type ObjectBytes = (Vec<u8>, Vec<[u8; 32]>);

fn element_objects(
    el: &Element,
    page: &Page,
    media: &mut MediaTable,
    now: u64,
) -> Vec<ObjectBytes> {
    if let Some(raw) = el.raw() {
        return vec![(raw.bytes.to_vec(), raw.hashes.clone())];
    }
    match el {
        Element::Stroke(s) => encode::encode_stroke(s, page.width, page.height, now)
            .into_iter()
            .map(|o| (o.bytes, o.hashes))
            .collect(),
        Element::Text(t) if t.text.trim().is_empty() => Vec::new(),
        Element::Text(t) => {
            let o = encode::encode_text(t, page.width, page.height, now);
            vec![(o.bytes, o.hashes)]
        }
        Element::Image(i) => {
            let bind = media.bind_for(i);
            let o = encode::encode_image(i.rect, bind, page.width, page.height, now);
            vec![(o.bytes, o.hashes)]
        }
        Element::Opaque(o) => vec![(o.raw.bytes.to_vec(), o.raw.hashes.clone())],
    }
}

fn drawn_rect(page: &Page) -> Option<Rect> {
    page.elements
        .iter()
        .filter(|e| !matches!(e, Element::Opaque(o) if o.hidden))
        .map(Element::bounds)
        .filter(|r| r.width() >= 0.0 && r.height() >= 0.0 && r.left.is_finite())
        .reduce(Rect::union)
}

/// Updates the drawn rectangle and drops stale rendered-page caches in a page's
/// header/property block. Only edits the block when every field could be walked.
fn adjust_properties(head: &mut Vec<u8>, drawn: Option<Rect>) {
    let (Some(prop), Some(mut mask)) = (rd_u32(head, 4), rd_u32(head, 0x0e)) else {
        return;
    };
    let prop = prop as usize;
    let walk = |head: &[u8], mask: u32| -> Option<(Option<(usize, usize)>, usize)> {
        let mut at = prop;
        if mask & 0x1 != 0 {
            at += 32;
        }
        if mask & 0x2 != 0 {
            let n = rd_u16(head, at)?;
            at += 2;
            for _ in 0..n {
                at = rd_str16(head, at)?.1;
            }
        }
        if mask & 0x4 != 0 {
            at = rd_str16(head, at)?.1;
        }
        for bit in [0x8, 0x10, 0x20, 0x40, 0x80] {
            if mask & bit != 0 {
                at += 4;
            }
        }
        if mask & 0x100 != 0 {
            at += 2 + 24 * rd_u16(head, at)? as usize;
        }
        if mask & 0x200 != 0 {
            at += 4;
        }
        let mut cache = None;
        if mask & 0x400 != 0 {
            let n = rd_u32(head, at)? as usize;
            let size = rd_u16(head, at + 4)? as usize;
            let start = at;
            at += 6 + n * size;
            cache = Some((start, at));
        }
        for bit in [0x800, 0x1000] {
            if mask & bit != 0 {
                at += 4;
            }
        }
        Some((cache, at))
    };
    let known = mask & !0x1FFF == 0;
    let walked = walk(head, mask).filter(|(_, end)| known && *end == head.len());

    if let Some((cache, _)) = walked {
        if let Some((start, end)) = cache {
            head.drain(start..end);
            mask &= !0x400;
        }
        if drawn.is_some() && mask & 0x1 == 0 {
            head.splice(prop..prop, std::iter::repeat_n(0u8, 32));
            mask |= 0x1;
        }
        head[0x0e..0x12].copy_from_slice(&mask.to_le_bytes());
    }
    if let Some(r) = drawn
        && mask & 0x1 != 0
        && head.len() >= prop + 32
    {
        for (i, v) in [r.left, r.top, r.right, r.bottom].into_iter().enumerate() {
            head[prop + i * 8..prop + i * 8 + 8].copy_from_slice(&(v as f64).to_le_bytes());
        }
    }
}

fn finish_page(
    mut content: Vec<u8>,
    layer_hashes: &[[u8; 32]],
    identity: (&str, u64),
) -> (Vec<u8>, [u8; 32]) {
    let mut buf = layer_hashes.concat();
    buf.extend_from_slice(&identity_hash(identity.0, identity.1));
    let hash = sha256(&buf);
    content.extend_from_slice(&hash);
    content.extend_from_slice(PAGE_FOOTER);
    (content, hash)
}

fn rebuild_page(
    page: &Page,
    src: &PageSource,
    media: &mut MediaTable,
    now: u64,
) -> (Vec<u8>, [u8; 32]) {
    let b = &src.bytes;
    let mut head = b[..src.layer_offset].to_vec();
    adjust_properties(&mut head, drawn_rect(page));
    let section_start = head.len();
    head[0..4].copy_from_slice(&(section_start as u32).to_le_bytes());

    let current_index = rd_u16(b, src.layer_offset + 2).unwrap_or(0);
    let mut out = head;
    out.extend_from_slice(&(src.layer_count as u16).to_le_bytes());
    out.extend_from_slice(&current_index.to_le_bytes());

    let mut layer_hashes = Vec::new();
    for (i, &(start, end)) in src.layer_ranges.iter().enumerate() {
        let abs = out.len();
        if i == src.current_layer {
            let objects: Vec<ObjectBytes> = page
                .elements
                .iter()
                .flat_map(|e| element_objects(e, page, media, now))
                .collect();
            let refs: Vec<(&[u8], &[[u8; 32]])> = objects
                .iter()
                .map(|(b, h)| (b.as_slice(), h.as_slice()))
                .collect();
            let (bytes, hash) = match &src.current_layer_identity {
                Some((id, t)) => {
                    let header_size = rd_u32(b, start).unwrap_or(0) as usize;
                    encode::layer_record(
                        Some((&b[start..start + header_size], start)),
                        (id, *t),
                        abs,
                        &refs,
                    )
                }
                None => encode::layer_record(None, (&crate::bin::samsung_uuid(), now), abs, &refs),
            };
            out.extend(bytes);
            layer_hashes.push(hash);
        } else {
            let mut layer = b[start..end].to_vec();
            let old_meta = rd_u32(&layer, 4).unwrap_or(0) as usize;
            let new_meta = abs + old_meta.saturating_sub(start);
            layer[4..8].copy_from_slice(&(new_meta as u32).to_le_bytes());
            layer_hashes.push(layer[layer.len() - 32..].try_into().unwrap());
            out.extend(layer);
        }
    }
    finish_page(out, &layer_hashes, (&src.identity.0, src.identity.1))
}

fn new_page(page: &Page, media: &mut MediaTable, now: u64) -> (Vec<u8>, [u8; 32]) {
    let mut out = encode::new_page_header(
        &page.id,
        page.width,
        page.height,
        page.background,
        drawn_rect(page),
        now,
    );
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    let abs = out.len();
    let objects: Vec<ObjectBytes> = page
        .elements
        .iter()
        .flat_map(|e| element_objects(e, page, media, now))
        .collect();
    let refs: Vec<(&[u8], &[[u8; 32]])> = objects
        .iter()
        .map(|(b, h)| (b.as_slice(), h.as_slice()))
        .collect();
    let (layer, hash) = encode::layer_record(None, (&crate::bin::samsung_uuid(), now), abs, &refs);
    out.extend(layer);
    finish_page(out, &[hash], (&page.id, now))
}

/// Patches width/height in an existing `note.note` and refreshes its hash.
fn patch_note_dims(bytes: &[u8], old: (u32, u32), new: (u32, u32)) -> Option<Vec<u8>> {
    let mut at = 4;
    at += 1 + *bytes.get(at)? as usize;
    at += 1 + *bytes.get(at)? as usize;
    at += 4;
    at = rd_str16(bytes, at)?.1;
    at += 4 + 8 + 8;
    if rd_u32(bytes, at)? != old.0 || rd_u32(bytes, at + 4)? != old.1 || bytes.len() < at + 8 + 32 {
        return None;
    }
    let mut out = bytes.to_vec();
    out[at..at + 4].copy_from_slice(&new.0.to_le_bytes());
    out[at + 4..at + 8].copy_from_slice(&new.1.to_le_bytes());
    let n = out.len();
    let hash = sha256(&out[..n - 32]);
    out[n - 32..].copy_from_slice(&hash);
    Some(out)
}

/// Serialises the note to `.sdocx` bytes.
pub fn save_bytes(note: &Note) -> Result<(Vec<u8>, SaveReport), Error> {
    let now = now_us();
    let mut report = SaveReport::default();
    let src = note.source.as_deref();
    if src.is_some_and(|s| s.encrypted) {
        return Err(Error::Encrypted);
    }

    let originals: Vec<MediaFile> = src.map(|s| s.media.clone()).unwrap_or_default();
    let mut media = MediaTable {
        next_bind: originals.iter().map(|m| m.bind_id + 1).max().unwrap_or(0),
        entries: originals,
        by_data: HashMap::new(),
        now,
    };

    let mut pages: Vec<Page> = note.pages.clone();
    if src.is_none() && pages.last().is_none_or(|p| !p.elements.is_empty()) {
        // Samsung Notes exports end with an empty page; Dietrich found imports need it.
        let last = pages.last();
        pages.push(Page::blank(
            last.map_or(Note::DEFAULT_WIDTH, |p| p.width),
            last.map_or(Note::DEFAULT_HEIGHT, |p| p.height),
            last.map_or(Note::DEFAULT_BACKGROUND, |p| p.background),
        ));
    }

    let mut page_files: Vec<(String, Vec<u8>, [u8; 32])> = Vec::new();
    for page in &pages {
        let (bytes, hash) = match &page.source {
            Some(s) if !page.dirty => {
                report.pages_copied += 1;
                (s.bytes.to_vec(), s.page_hash)
            }
            Some(s) => {
                report.pages_rebuilt += 1;
                rebuild_page(page, s, &mut media, now)
            }
            None => {
                report.pages_new += 1;
                new_page(page, &mut media, now)
            }
        };
        page_files.push((page.id.clone(), bytes, hash));
    }

    let vpad = src
        .and_then(|s| s.note_dims)
        .map_or(DEFAULT_NOTE_VERTICAL_PADDING, |d| d.2);
    let width = pages
        .iter()
        .map(|p| p.width)
        .max()
        .unwrap_or(Note::DEFAULT_WIDTH);
    let height: u32 =
        pages.iter().map(|p| p.height).sum::<u32>() + vpad * (pages.len().saturating_sub(1) as u32);

    let note_bytes = match (src, note.text_changed()) {
        (Some(s), false) if s.note_bytes.is_some() => {
            let original = s.note_bytes.as_ref().unwrap();
            match s.note_dims {
                Some((w, h, _)) if (w, h) != (width, height) => {
                    patch_note_dims(original, (w, h), (width, height))
                        .unwrap_or_else(|| original.to_vec())
                }
                _ => original.to_vec(),
            }
        }
        _ => {
            if src.is_some() {
                report.notes.push(
                    "Başlık veya not metni değiştiği için note.note yeniden yazıldı; metin biçimlendirmesi (kalın, renk, liste vb.) sadeleşti."
                        .into(),
                );
            }
            encode::note_file(&note.title, &note.body, width, height, vpad, now).0
        }
    };
    let note_hash: [u8; 32] = note_bytes[note_bytes.len() - 32..].try_into().unwrap();

    let ids: Vec<(String, [u8; 32])> = page_files
        .iter()
        .map(|(id, _, h)| (id.clone(), *h))
        .collect();
    let page_id_info = encode::page_id_info(&note_hash, &ids);
    let end_tag = src
        .and_then(|s| s.end_tag.as_ref().map(|e| e.to_vec()))
        .unwrap_or_else(|| encode::end_tag(width, height as f32, now));
    let media_info = encode::media_info(
        src.and_then(|s| s.media_info_version).unwrap_or(5304),
        &media
            .entries
            .iter()
            .filter_map(|m| m.raw_entry.clone())
            .collect::<Vec<_>>(),
    );

    let mut entries: Vec<(String, Vec<u8>)> = vec![("media/mediaInfo.dat".into(), media_info)];
    for m in &media.entries {
        entries.push((format!("media/{}", m.file_name), m.data.to_vec()));
    }
    for (id, bytes, _) in page_files {
        entries.push((format!("{id}.page"), bytes));
    }
    entries.push(("pageIdInfo.dat".into(), page_id_info));
    entries.push(("note.note".into(), note_bytes));
    entries.push(("end_tag.bin".into(), end_tag.clone()));
    if let Some(s) = src {
        for (name, data) in &s.entries {
            let handled = matches!(
                name.as_str(),
                "note.note" | "pageIdInfo.dat" | "end_tag.bin"
            ) || (name.ends_with(".page") && !name.contains('/'))
                || entries.iter().any(|(n, _)| n == name);
            if !handled {
                entries.push((name.clone(), data.to_vec()));
            }
        }
    }
    Ok((encode::write_zip(&entries, Some(&end_tag)), report))
}

/// Saves to `path` through a temporary file so a failed write never leaves a broken note.
pub fn save(note: &Note, path: impl AsRef<std::path::Path>) -> Result<SaveReport, Error> {
    let path = path.as_ref();
    let (bytes, report) = save_bytes(note)?;
    // Sanity check with the independent parser before touching the disk.
    crate::verify::verify_bytes(&bytes)?;
    let tmp = path.with_extension("sdocx.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(report)
}
