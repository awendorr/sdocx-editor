//! Round-trip tests: every written file is re-read by the independent `sdocx`
//! parser with Samsung's integrity checks enabled.

use std::sync::Arc;

use sdocx_edit::*;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

const FIXTURES: &[&str] = &[
    "mushroom.sdocx",
    "raven.sdocx",
    "flowers.sdocx",
    "title_strokes.sdocx",
];

fn count(note: &Note, f: impl Fn(&Element) -> bool) -> usize {
    note.pages
        .iter()
        .flat_map(|p| &p.elements)
        .filter(|e| f(e))
        .count()
}

fn strokes(note: &Note) -> usize {
    count(note, |e| matches!(e, Element::Stroke(_)))
}

fn assert_clean(bytes: &[u8]) -> VerifyReport {
    let r = verify_bytes(bytes).expect("written file must parse with matching hashes");
    assert_eq!(r.mismatched_hashes, 0, "{:?}", r.diagnostics);
    r
}

fn tiny_png() -> Vec<u8> {
    // 2x2 opaque red PNG.
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 2, 2);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[255, 0, 0, 255].repeat(4)).unwrap();
    }
    out
}

#[test]
fn unchanged_notes_round_trip_byte_for_byte_per_page() {
    for name in FIXTURES {
        let bytes = fixture(name);
        let before = verify_bytes(&bytes).unwrap();
        let note = open_bytes(&bytes).unwrap();
        let (out, report) = save_bytes(&note).unwrap();
        assert_eq!(report.pages_rebuilt + report.pages_new, 0, "{name}");
        let after = assert_clean(&out);
        assert_eq!(before.pages, after.pages, "{name}");
        assert_eq!(before.strokes, after.strokes, "{name}");
        assert_eq!(before.matched_hashes, after.matched_hashes, "{name}");
        let again = open_bytes(&out).unwrap();
        assert_eq!(again.title, note.title);
        assert_eq!(again.body, note.body);
    }
}

#[test]
fn rebuilding_every_page_without_changes_keeps_content_and_hashes() {
    for name in FIXTURES {
        let bytes = fixture(name);
        let before = verify_bytes(&bytes).unwrap();
        let mut note = open_bytes(&bytes).unwrap();
        for p in &mut note.pages {
            p.dirty = true;
        }
        let (out, report) = save_bytes(&note).unwrap();
        assert_eq!(report.pages_rebuilt, note.pages.len());
        let after = assert_clean(&out);
        assert_eq!(before.strokes, after.strokes, "{name}");
        assert_eq!(before.matched_hashes, after.matched_hashes, "{name}");
        let again = open_bytes(&out).unwrap();
        for (a, b) in note.pages.iter().zip(&again.pages) {
            assert_eq!(a.elements.len(), b.elements.len(), "{name}");
        }
    }
}

#[test]
fn added_stroke_is_saved_and_read_back() {
    for name in FIXTURES {
        let mut note = open_bytes(&fixture(name)).unwrap();
        let n = strokes(&note);
        let points: Vec<[f32; 2]> = (0..50)
            .map(|i| {
                [
                    100.0 + i as f32 * 3.3,
                    200.0 + (i as f32 * 0.3).sin() * 40.0,
                ]
            })
            .collect();
        let page = &mut note.pages[0];
        page.elements.push(Element::Stroke(Stroke {
            points: points.clone(),
            pressures: vec![0.6; points.len()],
            color: 0xFF20_40C0,
            width: 6.0,
            raw: None,
        }));
        page.dirty = true;
        let (out, _) = save_bytes(&note).unwrap();
        assert_clean(&out);
        let again = open_bytes(&out).unwrap();
        assert_eq!(strokes(&again), n + 1, "{name}");
        let Some(Element::Stroke(s)) = again.pages[0].elements.last() else {
            panic!("{name}: stroke missing")
        };
        assert_eq!(s.points.len(), points.len());
        for (a, b) in s.points.iter().zip(&points) {
            assert!(
                (a[0] - b[0]).abs() < 0.05 && (a[1] - b[1]).abs() < 0.05,
                "{a:?} vs {b:?}"
            );
        }
        assert_eq!(s.color & 0x00FF_FFFF, 0x0020_40C0);
        assert!((s.width - 6.0).abs() < 0.01);
    }
}

#[test]
fn deleting_moving_and_editing_existing_objects() {
    let mut note = open_bytes(&fixture("mushroom.sdocx")).unwrap();
    let total = strokes(&note);
    // page 0 has a stroke and an (unrotated) text box
    let page = &mut note.pages[0];
    let idx = page
        .elements
        .iter()
        .position(|e| matches!(e, Element::Stroke(_)))
        .unwrap();
    page.elements.remove(idx);
    let t = page
        .elements
        .iter_mut()
        .find(|e| matches!(e, Element::Text(_)))
        .unwrap();
    if let Element::Text(t) = t {
        t.text = "Merhaba dünya — çğıöşü".into();
        t.raw = None;
    }
    page.dirty = true;
    // page 2 has images: move one
    let page = &mut note.pages[2];
    let img = page
        .elements
        .iter_mut()
        .find(|e| matches!(e, Element::Image(_)))
        .unwrap();
    let old = img.bounds();
    img.translate(40.0, 25.0);
    page.dirty = true;

    let (out, _) = save_bytes(&note).unwrap();
    assert_clean(&out);
    let again = open_bytes(&out).unwrap();
    assert_eq!(strokes(&again), total - 1);
    let text = again.pages[0].elements.iter().find_map(|e| match e {
        Element::Text(t) => Some(t.text.clone()),
        _ => None,
    });
    assert_eq!(text.as_deref(), Some("Merhaba dünya — çğıöşü"));
    let moved: Vec<Rect> = again.pages[2]
        .elements
        .iter()
        .filter_map(|e| {
            if let Element::Image(i) = e {
                Some(i.rect)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(moved.len(), 2);
    assert!(moved.iter().any(|r| (r.left - old.left - 40.0).abs() < 0.5 && (r.top - old.top - 25.0).abs() < 0.5));
}

#[test]
fn body_and_title_edits_are_saved() {
    let mut note = open_bytes(&fixture("raven.sdocx")).unwrap();
    note.title = "Kuzgun".into();
    note.body = format!("{}\nYeni satır: İstanbul'da güneşli bir gün.", note.body);
    let (out, report) = save_bytes(&note).unwrap();
    assert!(!report.notes.is_empty());
    assert_clean(&out);
    let again = open_bytes(&out).unwrap();
    assert_eq!(again.title, "Kuzgun");
    assert_eq!(again.body, note.body);
    assert_eq!(again.pages.len(), note.pages.len());
}

#[test]
fn new_note_from_scratch() {
    let mut note = Note::new();
    note.title = "Yeni not".into();
    note.body = "Gövde metni".into();
    let page = &mut note.pages[0];
    page.elements.push(Element::Stroke(Stroke {
        points: vec![[10.0, 10.0], [200.0, 300.0], [400.0, 120.0]],
        pressures: vec![],
        color: 0xFF00_0000,
        width: 4.0,
        raw: None,
    }));
    page.elements.push(Element::Text(TextBox {
        rect: Rect::new(100.0, 500.0, 700.0, 700.0),
        text: "Kutu içinde yazı".into(),
        font_size: 12.0,
        color: 0xFF33_3333,
        raw: None,
    }));
    page.elements.push(Element::Image(Image {
        rect: Rect::new(100.0, 800.0, 300.0, 1000.0),
        data: Arc::new(tiny_png()),
        name: "kirmizi.png".into(),
        raw: None,
        source_bind_id: None,
    }));
    note.pages.push(Page::blank(1080, 1527, 0xFFFF_FFFF));
    note.pages[1].elements.push(Element::Stroke(Stroke {
        points: vec![[50.0, 50.0], [60.0, 70.0]],
        pressures: vec![0.2, 0.9],
        color: 0xFFFF_0000,
        width: 2.0,
        raw: None,
    }));

    let (out, report) = save_bytes(&note).unwrap();
    assert_eq!(
        report.pages_new, 3,
        "two pages plus the trailing blank page"
    );
    let r = assert_clean(&out);
    assert_eq!(r.unavailable_hashes, 0, "{:?}", r.diagnostics);
    let again = open_bytes(&out).unwrap();
    assert_eq!(again.title, "Yeni not");
    assert_eq!(again.body, "Gövde metni");
    assert_eq!(again.pages.len(), 3);
    assert_eq!(strokes(&again), 2);
    assert_eq!(
        count(
            &again,
            |e| matches!(e, Element::Text(t) if t.text == "Kutu içinde yazı")
        ),
        1
    );
    let img = again.pages[0].elements.iter().find_map(|e| {
        if let Element::Image(i) = e {
            Some(i)
        } else {
            None
        }
    });
    assert_eq!(img.map(|i| i.data.as_slice()), Some(tiny_png().as_slice()));

    // Saving the re-opened file again copies everything.
    let (out2, report2) = save_bytes(&again).unwrap();
    assert_eq!(report2.pages_copied, 3);
    assert_clean(&out2);
}

#[test]
fn added_and_removed_pages() {
    let mut note = open_bytes(&fixture("flowers.sdocx")).unwrap();
    let (w, h) = (note.pages[0].width, note.pages[0].height);
    note.pages.insert(1, Page::blank(w, h, 0xFFFF_FFFF));
    note.pages[1].elements.push(Element::Text(TextBox {
        rect: Rect::new(50.0, 50.0, 500.0, 200.0),
        text: "Eklenen sayfa".into(),
        font_size: 10.0,
        color: 0xFF00_0000,
        raw: None,
    }));
    let (out, _) = save_bytes(&note).unwrap();
    assert_clean(&out);
    let again = open_bytes(&out).unwrap();
    assert_eq!(again.pages.len(), note.pages.len());

    let mut fewer = again.clone();
    fewer.pages.remove(1);
    let (out, _) = save_bytes(&fewer).unwrap();
    assert_clean(&out);
    assert_eq!(open_bytes(&out).unwrap().pages.len(), note.pages.len() - 1);
}

#[test]
fn rotated_text_boxes_stay_untouched() {
    let note = open_bytes(&fixture("mushroom.sdocx")).unwrap();
    // "Amanita muscaria" on page 1 is rotated: it must be kept as an opaque object.
    assert!(
        note.pages[1]
            .elements
            .iter()
            .any(|e| matches!(e, Element::Opaque(o) if o.raw.object_type == 2))
    );
}

#[test]
fn empty_text_boxes_are_not_written() {
    let mut note = Note::new();
    note.pages[0].elements.push(Element::Text(TextBox {
        rect: Rect::new(10.0, 10.0, 200.0, 100.0),
        text: "  ".into(),
        font_size: 10.0,
        color: 0xFF00_0000,
        raw: None,
    }));
    let (out, _) = save_bytes(&note).unwrap();
    assert_clean(&out);
    assert_eq!(
        count(&open_bytes(&out).unwrap(), |e| matches!(
            e,
            Element::Text(_)
        )),
        0
    );
}
