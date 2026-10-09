//! Clip thumbnails for the Library and clip lists (lib/thumbnails.ts).
//!
//! Each is a JPEG ffmpeg pulls from the clip (`Core::thumbnails_generate`, cached on
//! disk too). They run one at a time in request order and are memoised for the
//! session; `forget` drops one after its clip was re-exported. Views read the
//! global `Thumbnails` entity and observe it to redraw as thumbnails land.
use crate::state;
use gpui_kit::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub enum Thumb {
    Loading,
    /// The JPEG, and its width / height when it could be read.
    Ready {
        path: PathBuf,
        aspect: Option<f32>,
    },
    /// No preview; asking again (`load`) retries, as the source may come back.
    Failed,
}

pub struct Thumbnails {
    thumbs: HashMap<String, Thumb>,
    /// Blurred copies for the run cards' backdrops; None until made, or when they can't be.
    backdrops: HashMap<PathBuf, Option<Arc<RenderImage>>>,
}

struct GlobalThumbnails(Entity<Thumbnails>);

impl Global for GlobalThumbnails {}

/// One ffmpeg at a time, in request order (tokio's mutex is fair).
static QUEUE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

impl Thumbnails {
    pub fn global(cx: &mut App) -> Entity<Self> {
        if let Some(global) = cx.try_global::<GlobalThumbnails>() {
            return global.0.clone();
        }
        let entity = cx.new(|_| Self {
            thumbs: HashMap::new(),
            backdrops: HashMap::new(),
        });
        cx.set_global(GlobalThumbnails(entity.clone()));
        entity
    }

    pub fn get(&self, video: &str) -> Option<&Thumb> {
        self.thumbs.get(video)
    }

    /// Queue `video`'s thumbnail (taken at `seek` seconds) unless it is ready or on its way.
    pub fn load(&mut self, video: &str, seek: Option<f64>, cx: &mut Context<Self>) {
        if matches!(
            self.thumbs.get(video),
            Some(Thumb::Loading | Thumb::Ready { .. })
        ) {
            return;
        }
        self.thumbs.insert(video.into(), Thumb::Loading);
        let core = state::core(cx);
        let path = video.to_string();
        let job = state::run(cx, async move {
            let _turn = QUEUE.lock().await;
            let thumb = core.thumbnails_generate(&path, seek).await?;
            Ok(thumb.map(|thumb| {
                let aspect = image::image_dimensions(&thumb)
                    .ok()
                    .map(|(w, h)| w as f32 / h as f32)
                    .filter(|ratio| ratio.is_finite() && *ratio > 0.0);
                (thumb, aspect)
            }))
        });
        let key = video.to_string();
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                // Forgotten meanwhile: a newer request owns the slot now.
                if this.thumbs.get(&key) != Some(&Thumb::Loading) {
                    return;
                }
                let thumb = match result {
                    Ok(Some((path, aspect))) => Thumb::Ready { path, aspect },
                    _ => Thumb::Failed,
                };
                this.thumbs.insert(key, thumb);
                cx.notify();
            });
        })
        .detach();
    }

    /// Drop a thumbnail from the session cache, after the video was re-exported.
    pub fn forget(&mut self, video: &str) {
        if let Some(Thumb::Ready { path, .. }) = self.thumbs.remove(video) {
            self.backdrops.remove(&path);
        }
    }

    /// The run card's backdrop for `thumb`: the CSS `scale-125 blur-2xl saturate-150
    /// opacity-60` copy. GPUI has no blur filter, so a small blurred copy is made
    /// once on a background thread and stretched; None until it is ready.
    pub fn backdrop(&mut self, thumb: &Path, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        if let Some(backdrop) = self.backdrops.get(thumb) {
            return backdrop.clone();
        }
        self.backdrops.insert(thumb.to_path_buf(), None);
        let path = thumb.to_path_buf();
        let task = cx.background_spawn(async move { blurred(&path) });
        let key = thumb.to_path_buf();
        cx.spawn(async move |this, cx| {
            let image = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.backdrops.contains_key(&key) && image.is_some() {
                    this.backdrops.insert(key, image);
                    cx.notify();
                }
            });
        })
        .detach();
        None
    }
}

/// A 48px blurred, saturated copy with 60% opacity baked in, in GPUI's BGRA.
fn blurred(path: &Path) -> Option<Arc<RenderImage>> {
    let small = image::open(path).ok()?.thumbnail(48, 48).to_rgba8();
    let mut out = image::imageops::blur(&small, 2.5);
    for pixel in out.pixels_mut() {
        let [r, g, b, _] = pixel.0.map(f32::from);
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let saturate = |c: f32| (luma + (c - luma) * 1.5).clamp(0.0, 255.0) as u8;
        pixel.0 = [saturate(b), saturate(g), saturate(r), 153];
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(out)])))
}
