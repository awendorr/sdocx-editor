//! Editable model, importer and writer for Samsung Notes `.sdocx` files.
//!
//! Reading and high-fidelity rendering use the [`sdocx`] crate. Writing is a Rust
//! port of the writer from the Dietrich project, extended so that untouched pages,
//! objects and media are copied from the source file byte for byte.

pub mod bin;
pub mod encode;
pub mod import;
pub mod model;
pub mod render;
pub mod save;
mod templates;
pub mod verify;

pub use import::{open, open_bytes};
pub use model::*;
pub use render::{
    PageSvg, Rasterizer, RenderMode, RgbaImage, export_pdf, render_pages, render_svgs,
};
pub use save::{SaveReport, save, save_bytes};
pub use verify::{VerifyReport, verify_bytes};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Dosya hatası: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Format(String),
    #[error(
        "Bu not kilitli (şifreli). Kilitli Samsung Notes notları açılamaz; önce telefonda kilidini kaldırıp yeniden dışa aktarın."
    )]
    Encrypted,
}
