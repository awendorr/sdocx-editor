//! Re-reads written files with the independent `sdocx` parser.

use crate::Error;

#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    pub pages: usize,
    pub strokes: usize,
    pub matched_hashes: usize,
    pub mismatched_hashes: usize,
    pub unavailable_hashes: usize,
    pub diagnostics: Vec<String>,
}

/// Parses `bytes` with Samsung's integrity checks enabled. Fails when the file
/// cannot be parsed or any stored hash disagrees with its content.
pub fn verify_bytes(bytes: &[u8]) -> Result<VerifyReport, Error> {
    let options = sdocx::ParseOptions { verify_integrity: true, ..Default::default() };
    let parsed = sdocx::parse_bytes_detailed_with_options(bytes, &options)
        .map_err(|e| Error::Format(format!("Yazılan dosya doğrulanamadı: {e}")))?;
    let mut r = VerifyReport {
        pages: parsed.document.pages.len(),
        strokes: parsed.document.pages.iter().map(|p| p.strokes().count()).sum(),
        ..Default::default()
    };
    if let Some(i) = &parsed.integrity {
        for c in [i.note, i.objects, i.layers, i.pages, i.manifest] {
            r.matched_hashes += c.matched;
            r.mismatched_hashes += c.mismatched;
            r.unavailable_hashes += c.unavailable;
        }
    }
    r.diagnostics = parsed.report.diagnostics.iter().map(|d| format!("{:?}: {}", d.code, d.message)).collect();
    if r.mismatched_hashes > 0 {
        return Err(Error::Format(format!(
            "Yazılan dosyada {} bütünlük özeti tutmuyor: {}",
            r.mismatched_hashes,
            r.diagnostics.join("; ")
        )));
    }
    Ok(r)
}
