fn main() {
    for a in std::env::args().skip(1) {
        match sdocx_edit::open(&a) {
            Ok(n) => {
                println!("=== {a}\n title={:?} body={} chars pages={}", n.title, n.body.chars().count(), n.pages.len());
                for w in &n.warnings { println!("  ! {w}"); }
                for (i, p) in n.pages.iter().enumerate() {
                    let src = p.source.as_ref().unwrap();
                    let mut k = std::collections::BTreeMap::new();
                    for e in &p.elements { *k.entry(match e { sdocx_edit::Element::Stroke(_) => "stroke".to_string(), sdocx_edit::Element::Text(t) => format!("text(fs={})", t.font_size), sdocx_edit::Element::Image(_) => "image".into(), sdocx_edit::Element::Opaque(o) => format!("opaque:{}", o.kind) }).or_insert(0) += 1; }
                    println!("  page {i} {}x{} layers={} lid={} {:?}", p.width, p.height, src.layer_count, src.current_layer_identity.is_some(), k);
                }
            }
            Err(e) => println!("=== {a}\n ERR {e}"),
        }
    }
}
