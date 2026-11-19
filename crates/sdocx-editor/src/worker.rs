//! Background page rendering so the window stays responsive.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use sdocx_edit::{Note, Rasterizer, RenderMode, RgbaImage};

/// Identifies what a page texture shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TexKey {
    pub page_version: u64,
    pub text_version: u64,
    pub mode_full: bool,
    pub inverted: bool,
    pub scale_q: u32,
}

pub struct Job {
    pub note: Arc<Note>,
    /// (page index, page id, key) for every page to render.
    pub pages: Vec<(usize, String, TexKey)>,
    pub mode: RenderMode,
    pub scale: f32,
    pub max_side: u32,
}

pub enum Output {
    Page {
        id: String,
        key: TexKey,
        image: RgbaImage,
    },
    Error(String),
    Done,
}

pub struct Worker {
    tx: Sender<Job>,
    pub rx: Receiver<Output>,
    pub busy: bool,
}

impl Worker {
    pub fn new(ctx: eframe::egui::Context) -> Self {
        let (job_tx, job_rx) = channel::<Job>();
        let (out_tx, out_rx) = channel::<Output>();
        std::thread::Builder::new()
            .name("sayfa-cizici".into())
            .spawn(move || {
                let raster = Rasterizer::default();
                while let Ok(mut job) = job_rx.recv() {
                    // Only the newest request matters.
                    while let Ok(newer) = job_rx.try_recv() {
                        job = newer;
                    }
                    let indices: Vec<usize> = job.pages.iter().map(|p| p.0).collect();
                    match sdocx_edit::render_pages(&job.note, job.mode, Some(&indices)) {
                        Ok(svgs) => {
                            for svg in svgs {
                                let Some((_, id, key)) = job.pages.iter().find(|p| p.0 == svg.page)
                                else {
                                    continue;
                                };
                                match raster.rasterize(&svg.svg, job.scale, job.max_side) {
                                    Ok(mut image) => {
                                        if key.inverted {
                                            for px in image.pixels.chunks_exact_mut(4) {
                                                let (r, g, b) = crate::app::invert_lightness(
                                                    px[0], px[1], px[2],
                                                );
                                                px[0] = r;
                                                px[1] = g;
                                                px[2] = b;
                                            }
                                        }
                                        let _ = out_tx.send(Output::Page {
                                            id: id.clone(),
                                            key: *key,
                                            image,
                                        });
                                    }
                                    Err(e) => {
                                        let _ = out_tx.send(Output::Error(e.to_string()));
                                    }
                                }
                                ctx.request_repaint();
                            }
                        }
                        Err(e) => {
                            let _ = out_tx.send(Output::Error(e.to_string()));
                        }
                    }
                    let _ = out_tx.send(Output::Done);
                    ctx.request_repaint();
                }
            })
            .expect("render thread");
        Self {
            tx: job_tx,
            rx: out_rx,
            busy: false,
        }
    }

    pub fn submit(&mut self, job: Job) {
        if self.tx.send(job).is_ok() {
            self.busy = true;
        }
    }
}
