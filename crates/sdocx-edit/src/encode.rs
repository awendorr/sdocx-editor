//! Encoders for Samsung Notes binary records.
//!
//! This is a Rust port of the `.sdocx` writer in the Dietrich project
//! (MIT licence, https://github.com/Downhillghost/Dietrich,
//! `note_pipeline/output/samsung_notes.py`). Field layouts and constants follow
//! that implementation; see its `sdocx_specification/` folder for the format notes.

use crate::bin::{Buf, identity_hash, sha256};
use crate::model::{Argb, Rect, Stroke, TextBox};
use crate::templates::{
    CURRENT_PEN_INFO_TEMPLATE, EMPTY_BODY_OBJECT_TEMPLATE, TITLE_OBJECT_PREFIX,
};

pub const PAGE_FOOTER: &[u8] = b"Page for SAMSUNG S-Pen SDK";
pub const END_TAG_FOOTER: &[u8] = b"Document for S-Pen SDK";
const MAX_STROKE_POINTS: usize = 60_000;
const NOTE_PROPERTY_FLAGS: u32 = 0x000C_8E80;
const NOTE_HEADER_FLAGS: u32 = 0x0000_0008;
pub const DEFAULT_NOTE_VERTICAL_PADDING: u32 = 8;
const DEFAULT_PEN_NAME: &str = "com.samsung.android.sdk.pen.pen.preload.FountainPen";
const DEFAULT_ADVANCED_SETTING: &str = "18;0;100;";
const TEXT_FIELD_MARGINS: [f32; 4] = [8.0, 4.0, 8.0, 4.0];
const TITLE_OBJECT_TRAILER: &[u8] = &[0x0f, 0, 0, 0, 0x02, 0, 0, 0, 0, 0, 0x01, 0, 0x02, 0, 0];
const TEXT_FIELD_SHAPE_BASE_TAIL: &[u8] = &[
    0x04, 0, 0, 0, 0, 0, 0, 0, 0, 0x13, 0, 0, 0, 0x01, 0, 0x02, 0, 0, 0, 0xff, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0x0c, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
const TEXT_FIELD_TRAILER_SUBRECORD: &[u8] =
    &[0x0f, 0, 0, 0, 0x02, 0, 0, 0, 0, 0, 0x01, 0, 0x02, 0, 0];
const IMAGE_TRAILER_SUBRECORD: &[u8] = &[
    0x11, 0, 0, 0, 0x03, 0, 0, 0, 0, 0, 0x01, 0, 0x04, 0, 0, 0, 0,
];

/// One encoded page object: the full record and its integrity trailer.
pub struct EncodedObject {
    pub bytes: Vec<u8>,
    pub hashes: Vec<[u8; 32]>,
}

fn object_id_bytes(id: &str) -> [u8; 36] {
    let mut out = [b'0'; 36];
    for (o, b) in out.iter_mut().zip(id.bytes().filter(u8::is_ascii)) {
        *o = b;
    }
    out
}

fn finish_object(object_type: u8, id: &str, now: u64, payload: &[u8]) -> EncodedObject {
    let hash = identity_hash(id, now);
    let mut b = Buf::new();
    b.u8(object_type)
        .u16(0)
        .u32((payload.len() + 32) as u32)
        .bytes(payload)
        .bytes(&hash);
    EncodedObject {
        bytes: b.into_inner(),
        hashes: vec![hash],
    }
}

fn base_record(
    id: &str,
    rect: Rect,
    page_w: u32,
    page_h: u32,
    now: u64,
    fixed_point_dims: bool,
) -> Vec<u8> {
    let mut r = Buf::new();
    r.u32(121)
        .u16(0)
        .u32(105)
        .bytes(&[0x02, 0xbf, 0x01, 0x04, 0x00, 0x60, 0x00, 0x00])
        .u32(4000)
        .u16(36);
    r.bytes(&object_id_bytes(id)).u64(now);
    for v in [rect.left, rect.top, rect.right, rect.bottom] {
        r.f64(v as f64);
    }
    r.u32(0);
    if fixed_point_dims {
        r.u64(now)
            .u32(page_w.max(1) * 256)
            .u32(page_h.max(1) * 256)
            .u8(0);
    } else {
        r.u8(0).u64(now).u32(page_w.max(1)).u32(page_h.max(1));
    }
    r.into_inner()
}

fn pack_compact_delta(delta: f32) -> Option<u16> {
    let value = delta.abs() as f64;
    if !value.is_finite() || value >= 1024.0 {
        return None;
    }
    let mut integer = value.floor() as u32;
    let mut fraction = ((value - integer as f64) * 32.0).round() as u32;
    if fraction >= 32 {
        integer += 1;
        fraction = 0;
    }
    if integer >= 1024 {
        return None;
    }
    Some((if delta < 0.0 { 0x8000 } else { 0 }) | (integer << 5) as u16 | fraction as u16)
}

/// Value a packed delta decodes to.
fn unpack_compact_delta(v: u16) -> f32 {
    let magnitude = ((v >> 5) & 0x3FF) as f32 + (v & 0x1F) as f32 / 32.0;
    if v & 0x8000 != 0 {
        -magnitude
    } else {
        magnitude
    }
}

/// Packs a delta and advances `current` by what a reader will decode, so the
/// quantisation error does not accumulate along the stroke (unlike Dietrich).
fn push_delta(b: &mut Buf, target: f32, current: &mut f32, scale: f32) -> Option<()> {
    let packed = pack_compact_delta((target - *current) * scale)?;
    *current += unpack_compact_delta(packed) / scale;
    b.u16(packed);
    Some(())
}

fn pack_compact_series(values: &[f32], scale: f32) -> Option<Vec<u8>> {
    let mut b = Buf::new();
    let Some(&first) = values.first() else {
        return Some(Vec::new());
    };
    b.f32(first);
    let mut current = first;
    for &v in &values[1..] {
        push_delta(&mut b, v, &mut current, scale)?;
    }
    Some(b.into_inner())
}

fn pack_compact_timestamps(values: &[i32]) -> Option<Vec<u8>> {
    let mut b = Buf::new();
    let Some(&first) = values.first() else {
        return Some(Vec::new());
    };
    b.i32(first);
    let mut current = first;
    for &v in &values[1..] {
        let d = v - current;
        if !(0..=0xFFFF).contains(&d) {
            return None;
        }
        b.u16(d as u16);
        current = v;
    }
    Some(b.into_inner())
}

fn compact_geometry(points: &[[f32; 2]], pressures: &[f32], timestamps: &[i32]) -> Option<Vec<u8>> {
    let mut b = Buf::new();
    let [mut cx, mut cy] = points[0];
    b.f64(cx as f64).f64(cy as f64);
    for p in &points[1..] {
        push_delta(&mut b, p[0], &mut cx, 1.0)?;
        push_delta(&mut b, p[1], &mut cy, 1.0)?;
    }
    let zeros = vec![0.0; points.len()];
    b.bytes(&pack_compact_series(pressures, 128.0)?);
    b.bytes(&pack_compact_timestamps(timestamps)?);
    b.bytes(&pack_compact_series(&zeros, 32.0)?);
    b.bytes(&pack_compact_series(&zeros, 32.0)?);
    b.u16(3);
    Some(b.into_inner())
}

fn raw_geometry(points: &[[f32; 2]], pressures: &[f32], timestamps: &[i32]) -> Vec<u8> {
    let mut b = Buf::new();
    for p in points {
        b.f32(p[0]).f32(p[1]);
    }
    for &p in pressures {
        b.f32(p);
    }
    for &t in timestamps {
        b.i32(t);
    }
    b.u16(3);
    b.into_inner()
}

fn encode_stroke_chunk(
    points: &[[f32; 2]],
    pressures: &[f32],
    color: Argb,
    width: f32,
    page_w: u32,
    page_h: u32,
    now: u64,
) -> EncodedObject {
    let id = crate::bin::samsung_uuid();
    let pressures: Vec<f32> = if pressures.len() == points.len() {
        pressures
            .iter()
            .map(|p| {
                if p.is_finite() {
                    p.clamp(0.0, 1.0)
                } else {
                    0.5
                }
            })
            .collect()
    } else {
        vec![0.5; points.len()]
    };
    let timestamps: Vec<i32> = (0..points.len() as i32).map(|i| i * 8).collect();
    let compact = compact_geometry(points, &pressures, &timestamps);
    let mask1: u16 = if compact.is_some() { 0x0425 } else { 0 };
    let geometry = compact.unwrap_or_else(|| raw_geometry(points, &pressures, &timestamps));

    let mut inner = Buf::new();
    inner
        .u8(2)
        .u16(mask1)
        .u8(4)
        .u32(0x258E)
        .u16(points.len() as u16)
        .bytes(&geometry);

    let pen = width.max(0.5);
    let mut flexible = Buf::new();
    flexible
        .u32(1)
        .u32(color)
        .f32(pen)
        .u32(0)
        .f32(pen)
        .u32(1)
        .f32((pen - 2.5).max(0.1));

    let mut body = Buf::new();
    body.u32((6 + 4 + inner.len()) as u32)
        .bytes(&inner.0)
        .bytes(&flexible.0);
    let mut stroke_sub = Buf::new();
    stroke_sub
        .u32((body.len() + 6) as u32)
        .u16(1)
        .bytes(&body.0);

    let pad = pen.max(1.0);
    let mut bounds = Rect {
        left: f32::MAX,
        top: f32::MAX,
        right: f32::MIN,
        bottom: f32::MIN,
    };
    for p in points {
        bounds.left = bounds.left.min(p[0]);
        bounds.top = bounds.top.min(p[1]);
        bounds.right = bounds.right.max(p[0]);
        bounds.bottom = bounds.bottom.max(p[1]);
    }
    let mut payload = base_record(&id, bounds.expand(pad), page_w, page_h, now, true);
    payload.extend_from_slice(&stroke_sub.0);
    finish_object(1, &id, now, &payload)
}

/// Encodes a stroke; strokes longer than Samsung's limit become several objects.
pub fn encode_stroke(s: &Stroke, page_w: u32, page_h: u32, now: u64) -> Vec<EncodedObject> {
    let points: Vec<[f32; 2]> = s
        .points
        .iter()
        .copied()
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .collect();
    let mut out = Vec::new();
    if points.len() < 2 {
        // A single tap: duplicate the point so it is a valid two-point stroke.
        if let Some(&p) = points.first() {
            out.push(encode_stroke_chunk(
                &[p, p],
                &[0.5, 0.5],
                s.color,
                s.width,
                page_w,
                page_h,
                now,
            ));
        }
        return out;
    }
    let pressures = if s.pressures.len() == s.points.len() {
        s.pressures.clone()
    } else {
        Vec::new()
    };
    let mut start = 0;
    while start < points.len() {
        let end = (start + MAX_STROKE_POINTS).min(points.len());
        if end - start >= 2 {
            let pr = if pressures.is_empty() {
                &[][..]
            } else {
                &pressures[start..end]
            };
            out.push(encode_stroke_chunk(
                &points[start..end],
                pr,
                s.color,
                s.width,
                page_w,
                page_h,
                now,
            ));
        }
        start = end;
    }
    out
}

fn path_block(rect: Rect) -> Vec<u8> {
    let mut b = Buf::new();
    b.u32(73).u32(5);
    for (cmd, x, y) in [
        (1u8, rect.left, rect.top),
        (2, rect.right, rect.top),
        (2, rect.right, rect.bottom),
        (2, rect.left, rect.bottom),
    ] {
        b.u8(cmd).f64(x as f64).f64(y as f64);
    }
    b.u16(6);
    b.into_inner()
}

fn shape_layout_record(rect: Rect, tail: &[u8]) -> Vec<u8> {
    let mid_x = ((rect.left + rect.right) / 2.0) as f64;
    let mid_y = ((rect.top + rect.bottom) / 2.0) as f64;
    let mut r = Buf::new();
    r.u32(130)
        .u16(6)
        .u32(91)
        .bytes(&[0x01, 0x00, 0x01, 0x0c])
        .u32(4);
    for v in [
        mid_x,
        rect.top as f64,
        rect.right as f64,
        mid_y,
        mid_x,
        rect.bottom as f64,
        rect.left as f64,
        mid_y,
    ] {
        r.f64(v);
    }
    r.bytes(tail);
    r.into_inner()
}

fn span_record(span_type: u32, start: u32, end: u32, extra: &[u8]) -> Vec<u8> {
    let mut p = Buf::new();
    p.u32(span_type).u32(start).u32(end).u32(1).bytes(extra);
    p.0.resize(24, 0);
    let mut b = Buf::new();
    b.u16(24).bytes(&p.0);
    b.into_inner()
}

fn paragraph_record(kind: u32, start: u32, end: u32, extra: &[u8]) -> Vec<u8> {
    let mut p = Buf::new();
    p.u32(kind).u32(start).u32(end).bytes(extra);
    p.0.resize(20, 0);
    let mut b = Buf::new();
    b.u16(20).bytes(&p.0);
    b.into_inner()
}

fn text_field_common(t: &TextBox) -> Vec<u8> {
    let units: Vec<u16> = t.text.encode_utf16().collect();
    let n = units.len() as u32;
    let mut b = Buf::new();
    b.u32(n);
    for u in &units {
        b.u16(*u);
    }
    b.u32(2);
    b.bytes(&span_record(3, 0, n, &t.font_size.max(1.0).to_le_bytes()));
    b.bytes(&span_record(1, 0, n, &t.color.to_le_bytes()));
    b.u32(2);
    b.bytes(&paragraph_record(3, 0, 1, &0u32.to_le_bytes()));
    b.bytes(&paragraph_record(6, 0, 1, &0u32.to_le_bytes()));
    for m in TEXT_FIELD_MARGINS {
        b.f32(m);
    }
    b.u8(0).u16(0).u32(0).u32(0);
    b.into_inner()
}

/// Encodes a free text box (Samsung "text field" object).
pub fn encode_text(t: &TextBox, page_w: u32, page_h: u32, now: u64) -> EncodedObject {
    let id = crate::bin::samsung_uuid();
    let rect = t.rect;
    let common = text_field_common(t);
    let mut body = Buf::new();
    body.u32(135).bytes(&[0x01, 0x04, 0x04]).u32(0x2001).u32(4);
    for v in [rect.left, rect.top, rect.right, rect.bottom] {
        body.f64(v as f64);
    }
    body.u32(0)
        .bytes(&path_block(rect))
        .u32(common.len() as u32)
        .bytes(&common)
        .u8(2);
    let mut text_rec = Buf::new();
    text_rec.u32((body.len() + 6) as u32).u16(7).bytes(&body.0);

    let mut payload = base_record(&id, rect, page_w, page_h, now, false);
    payload.extend(shape_layout_record(rect, TEXT_FIELD_SHAPE_BASE_TAIL));
    payload.extend(text_rec.0);
    payload.extend_from_slice(TEXT_FIELD_TRAILER_SUBRECORD);
    finish_object(2, &id, now, &payload)
}

/// Encodes an inserted image that refers to media `bind_id`.
pub fn encode_image(rect: Rect, bind_id: u32, page_w: u32, page_h: u32, now: u64) -> EncodedObject {
    let id = crate::bin::samsung_uuid();
    let mut base = Buf::new();
    base.u32(122)
        .u16(0)
        .u32(105)
        .bytes(&[0x02, 0xbf, 0x01, 0x04, 0x00, 0xe0, 0x00, 0x00])
        .u32(4000)
        .u16(36);
    base.bytes(&object_id_bytes(&id)).u64(now);
    for v in [rect.left, rect.top, rect.right, rect.bottom] {
        base.f64(v as f64);
    }
    base.u32(0)
        .u64(now)
        .u32(page_w.max(1) * 256)
        .u32(page_h.max(1) * 256)
        .bytes(&[0x00, 0x02]);

    let mut layout_tail = Buf::new();
    layout_tail
        .u32(4)
        .zeros(5)
        .bytes(&[0x13, 0, 0, 0, 0x01, 0, 0x02, 0, 0, 0, 0xff, 0, 0, 0])
        .zeros(9)
        .u32(12)
        .zeros(12);

    let mut fill = Buf::new();
    fill.u32(62)
        .u8(2)
        .u8(0)
        .u32(bind_id)
        .zeros(16)
        .zeros(8)
        .f32(100.0)
        .f32(100.0)
        .f32(0.0)
        .u8(0)
        .zeros(16);
    fill.u32(page_w.max(1)).u8(2);

    let mut shape = Buf::new();
    shape
        .u32(203)
        .u16(7)
        .u32(135)
        .bytes(&[0x01, 0x00, 0x04, 0x20, 0x10, 0x00, 0x00])
        .u32(4);
    for v in [rect.left, rect.top, rect.right, rect.bottom] {
        shape.f64(v as f64);
    }
    shape.u32(0).bytes(&path_block(rect)).bytes(&fill.0);

    let mut payload = base.into_inner();
    payload.extend(shape_layout_record(rect, &layout_tail.0));
    payload.extend(shape.0);
    payload.extend_from_slice(IMAGE_TRAILER_SUBRECORD);
    finish_object(3, &id, now, &payload)
}

/// Builds a layer record. Returns the bytes and the layer hash.
pub fn layer_record(
    header: Option<(&[u8], usize)>,
    identity: (&str, u64),
    abs_offset: usize,
    objects: &[(&[u8], &[[u8; 32]])],
) -> (Vec<u8>, [u8; 32]) {
    let mut layer = Buf::new();
    match header {
        Some((h, old_offset)) => {
            // Original header: move the absolute metadata offset with the layer.
            let mut h = h.to_vec();
            let old_meta = u32::from_le_bytes(h[4..8].try_into().unwrap()) as usize;
            let new_meta = abs_offset + old_meta.saturating_sub(old_offset);
            h[4..8].copy_from_slice(&(new_meta as u32).to_le_bytes());
            layer.bytes(&h);
        }
        None => {
            let mut extra = Buf::new();
            extra.str16(identity.0).u64(identity.1);
            layer
                .u32((16 + extra.len()) as u32)
                .u32((abs_offset + 16) as u32)
                .bytes(&[1, 2, 1, 0x18])
                .u32(0);
            layer.bytes(&extra.0);
        }
    }
    layer.u32(objects.len() as u32);
    let mut hashes = Vec::new();
    for (bytes, h) in objects {
        layer.bytes(bytes);
        for x in *h {
            hashes.extend_from_slice(x);
        }
    }
    hashes.extend_from_slice(&identity_hash(identity.0, identity.1));
    let hash = sha256(&hashes);
    layer.bytes(&hash);
    (layer.into_inner(), hash)
}

/// Header and property block of a brand-new page (Dietrich layout).
pub fn new_page_header(
    id: &str,
    width: u32,
    height: u32,
    background: Argb,
    drawn: Option<Rect>,
    now: u64,
) -> Vec<u8> {
    let mut p = Buf::new();
    p.zeros(18)
        .u32(0)
        .u32(width)
        .u32(height)
        .u32(0)
        .u32(0)
        .str16(id)
        .u64(now)
        .u32(4000)
        .u32(4000);
    p.0.resize(0x80, 0);
    if let Some(r) = drawn {
        for v in [r.left, r.top, r.right, r.bottom] {
            p.f64(v as f64);
        }
    }
    p.u32(2).u32(background).u32(width);
    let layer_offset = p.len() as u32;
    let mask = 0x70 | if drawn.is_some() { 1 } else { 0 };
    p.put_u32_at(0, layer_offset);
    p.put_u32_at(4, 0x80);
    p.0[8] = 4;
    p.put_u32_at(9, 0);
    p.0[0x0d] = 4;
    p.put_u32_at(0x0e, mask);
    p.into_inner()
}

fn title_text_common(title: &str) -> Vec<u8> {
    let text: String = title.chars().take(240).collect();
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut p = Buf::new();
    p.u32(units.len() as u32);
    for u in &units {
        p.u16(*u);
    }
    p.u32(0);
    if text.is_empty() {
        p.u32(0);
    } else {
        p.u32(1).u16(20).u32(6).u32(0).u32(1).u32(0).u32(0);
    }
    for v in [15.0f32, 0.0, 15.0, 0.0] {
        p.f32(v);
    }
    p.u8(0).u16(0).u32(0).u32(0);
    p.into_inner()
}

fn patch_text_object_id(data: &mut [u8], id: &str, now: u64) {
    data[24..60].copy_from_slice(&object_id_bytes(id));
    data[60..68].copy_from_slice(&now.to_le_bytes());
}

fn title_object(title: &str, now: u64) -> Vec<u8> {
    let mut data = TITLE_OBJECT_PREFIX.to_vec();
    patch_text_object_id(&mut data, &crate::bin::samsung_uuid(), now);
    let common = title_text_common(title);
    data[179..183].copy_from_slice(&((66 + common.len()) as u32).to_le_bytes());
    data.extend_from_slice(&(common.len() as u32).to_le_bytes());
    data.extend(common);
    data.extend_from_slice(TITLE_OBJECT_TRAILER);
    data
}

/// Body object: the empty-body template with its text replaced.
fn body_object(body: &str, now: u64) -> Vec<u8> {
    let mut data = EMPTY_BODY_OBJECT_TEMPLATE[..241].to_vec();
    patch_text_object_id(&mut data, &crate::bin::samsung_uuid(), now);
    let units: Vec<u16> = body.encode_utf16().collect();
    let mut common = Buf::new();
    common.u32(units.len() as u32);
    for u in &units {
        common.u16(*u);
    }
    common.u32(0).u32(0);
    for v in [16.0f32, 10.0, 16.0, 10.0] {
        common.f32(v);
    }
    common.u8(0).u16(0).u32(0).u32(0);
    data[179..183].copy_from_slice(&((66 + common.len() + 1) as u32).to_le_bytes());
    data.extend_from_slice(&(common.len() as u32).to_le_bytes());
    data.extend(common.0);
    data.push(2);
    data.extend_from_slice(&EMPTY_BODY_OBJECT_TEMPLATE[EMPTY_BODY_OBJECT_TEMPLATE.len() - 15..]);
    data
}

/// Builds `note.note` with the given title and body text. Returns bytes and note hash.
pub fn note_file(
    title: &str,
    body: &str,
    width: u32,
    height: u32,
    vpad: u32,
    now: u64,
) -> (Vec<u8>, [u8; 32]) {
    let title_obj = title_object(title, now);
    let body_obj = body_object(body, now);
    let mut d = Buf::new();
    d.zeros(4)
        .u8(4)
        .u32(NOTE_HEADER_FLAGS)
        .u8(4)
        .u32(NOTE_PROPERTY_FLAGS)
        .u32(4000)
        .str16("")
        .u32(5);
    d.u64(now)
        .u64(now)
        .u32(width)
        .u32(height)
        .u32(0)
        .u32(vpad)
        .u32(4000);
    d.u32(title_obj.len() as u32).bytes(&title_obj);
    d.u32(body_obj.len() as u32).bytes(&body_obj);
    let optional = d.len() as u32;
    d.put_u32_at(0, optional);
    d.i32(0).i32(-1).u64(0);
    let mut strings = Buf::new();
    strings
        .u16(2)
        .i32(0)
        .str16(DEFAULT_PEN_NAME)
        .i32(1)
        .str16(DEFAULT_ADVANCED_SETTING);
    d.u32(strings.len() as u32).bytes(&strings.0);
    d.i32(0).bytes(CURRENT_PEN_INFO_TEMPLATE).i32(2).i32(2);
    let hash = sha256(&d.0);
    d.bytes(&hash);
    (d.into_inner(), hash)
}

pub fn page_id_info(note_hash: &[u8; 32], pages: &[(String, [u8; 32])]) -> Vec<u8> {
    let mut d = Buf::new();
    d.bytes(note_hash).u16(pages.len() as u16);
    for (id, h) in pages {
        d.str16(id).bytes(h);
    }
    d.into_inner()
}

pub fn end_tag(width: u32, height: f32, now: u64) -> Vec<u8> {
    let mut p = Buf::new();
    p.i32(4000)
        .str16("")
        .u64(now)
        .i32(0)
        .str16("")
        .i32(width as i32)
        .f32(height)
        .str16("");
    p.i32(-1)
        .i32(-1)
        .str16("")
        .i32(4000)
        .u64(now)
        .i32(0)
        .u16(0)
        .u16(0)
        .str16("")
        .i32(0)
        .i32(0);
    p.u64(now)
        .u64(now / 1000)
        .u64(0)
        .str16("")
        .i32(2)
        .i32(2)
        .i64(-1)
        .i32(0)
        .i32(0)
        .str32("");
    p.bytes(END_TAG_FOOTER);
    let mut d = Buf::new();
    d.u16(p.len() as u16).bytes(&p.0);
    d.into_inner()
}

/// `mediaInfo.dat` body for a new entry.
pub fn media_entry(bind_id: u32, file_name: &str, data: &[u8], now: u64) -> Vec<u8> {
    let mut b = Buf::new();
    b.u32(bind_id)
        .str16(file_name)
        .bytes(crate::bin::hex(&sha256(data)).as_bytes())
        .u16(1)
        .u64(now)
        .u8(1);
    b.into_inner()
}

pub fn media_info(version: u32, entries: &[Vec<u8>]) -> Vec<u8> {
    let mut d = Buf::new();
    d.u32(version).u16(entries.len() as u16);
    for e in entries {
        d.u32(e.len() as u32).bytes(e);
    }
    d.bytes(b"EOFX");
    d.into_inner()
}

/// Writes a ZIP the way Samsung Notes does: deflated entries, then the raw end
/// tag appended after the end-of-central-directory record.
pub fn write_zip(entries: &[(String, Vec<u8>)], trailing: Option<&[u8]>) -> Vec<u8> {
    use std::io::Write;
    const FLAGS: u16 = 0x0806;
    const DOS_DATE: u16 = 0x0020; // what Samsung Notes writes (1980, month 1, day 0)
    let mut out = Buf::new();
    let mut central = Buf::new();
    for (name, data) in entries {
        let mut enc = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(data).expect("in-memory write");
        let compressed = enc.finish().expect("in-memory write");
        let crc = crc32fast::hash(data);
        let offset = out.len() as u32;
        let name_b = name.as_bytes();
        out.u32(0x0403_4B50)
            .u16(20)
            .u16(FLAGS)
            .u16(8)
            .u16(0)
            .u16(DOS_DATE)
            .u32(crc);
        out.u32(compressed.len() as u32)
            .u32(data.len() as u32)
            .u16(name_b.len() as u16)
            .u16(0);
        out.bytes(name_b).bytes(&compressed);
        central
            .u32(0x0201_4B50)
            .u16(20)
            .u16(20)
            .u16(FLAGS)
            .u16(8)
            .u16(0)
            .u16(DOS_DATE)
            .u32(crc);
        central
            .u32(compressed.len() as u32)
            .u32(data.len() as u32)
            .u16(name_b.len() as u16);
        central
            .u16(0)
            .u16(0)
            .u16(0)
            .u16(0)
            .u32(0)
            .u32(offset)
            .bytes(name_b);
    }
    let cd_offset = out.len() as u32;
    out.bytes(&central.0);
    out.u32(0x0605_4B50)
        .u16(0)
        .u16(0)
        .u16(entries.len() as u16)
        .u16(entries.len() as u16);
    out.u32(central.len() as u32).u32(cd_offset).u16(0);
    if let Some(t) = trailing {
        out.bytes(t);
    }
    out.into_inner()
}
