//! Renders a note to PNG files: `cargo run --example render -- in.sdocx outdir [underlay]`
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let note = sdocx_edit::open(&a[1]).unwrap();
    let mode = if a.get(3).is_some() {
        sdocx_edit::RenderMode::Underlay
    } else {
        sdocx_edit::RenderMode::Full
    };
    std::fs::create_dir_all(&a[2]).unwrap();
    let r = sdocx_edit::Rasterizer::default();
    for p in sdocx_edit::render_svgs(&note, mode).unwrap() {
        let img = r.rasterize(&p.svg, 0.5, 4096).unwrap();
        let f = std::fs::File::create(format!("{}/page{}.png", a[2], p.page)).unwrap();
        let mut e = png::Encoder::new(f, img.width, img.height);
        e.set_color(png::ColorType::Rgba);
        e.write_header()
            .unwrap()
            .write_image_data(&img.pixels)
            .unwrap();
    }
    std::fs::write(
        format!("{}/note.pdf", a[2]),
        sdocx_edit::export_pdf(&note).unwrap(),
    )
    .unwrap();
}
