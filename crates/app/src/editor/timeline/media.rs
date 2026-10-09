//! Filmstrip thumbnails and loudness peaks (Filmstrip.tsx, Waveform.tsx), kept for the
//! session and shared by every timeline.
//!
//! A filmstrip block is one FFmpeg run, so blocks load one at a time, the ones on
//! screen first; each block's image is cut into its one-per-second frames here. Peaks
//! load as asked. A failed read may work later (a file still importing), so it is
//! retried when a timeline asks again after a while.
use crate::state;
use clipward_core::clip_engine::render::project::FILMSTRIP_BLOCK_S;
use gpui_kit::*;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a failed read stays failed before a timeline may ask again.
const RETRY_AFTER: Duration = Duration::from_secs(10);
/// Height an added image's thumbnail is kept at (the filmstrip's frame height).
const IMAGE_HEIGHT: u32 = 180;

/// Run folder, file (None for the source) and block.
pub type StripKey = (String, Option<String>, u32);
/// Run folder and file (None for the source).
pub type FileKey = (String, Option<String>);
/// A block's frames, one per second.
pub type Frames = Arc<Vec<Arc<RenderImage>>>;
pub type Peaks = Arc<Vec<u8>>;

enum Load<T> {
    Pending,
    Ready(T),
    Failed(Instant),
}

impl<T> Load<T> {
    fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(value) => Some(value),
            _ => None,
        }
    }
}

/// Whether to ask for something in this state.
fn wanted<T>(load: Option<&Load<T>>) -> bool {
    match load {
        None => true,
        Some(Load::Failed(at)) => at.elapsed() >= RETRY_AFTER,
        Some(_) => false,
    }
}

#[derive(Default)]
pub struct MediaCache {
    strips: HashMap<StripKey, Load<Frames>>,
    queue: VecDeque<StripKey>,
    busy: bool,
    peaks: HashMap<FileKey, Load<Peaks>>,
    images: HashMap<FileKey, Load<Arc<RenderImage>>>,
}

impl Global for MediaCache {}

impl MediaCache {
    pub fn strip(&self, key: &StripKey) -> Option<&Frames> {
        self.strips.get(key).and_then(Load::ready)
    }

    pub fn peaks(&self, key: &FileKey) -> Option<&Peaks> {
        self.peaks.get(key).and_then(Load::ready)
    }

    pub fn image(&self, key: &FileKey) -> Option<&Arc<RenderImage>> {
        self.images.get(key).and_then(Load::ready)
    }
}

/// What a render needs that isn't loaded; `request` it once the frame is built.
#[derive(Default)]
pub struct Wants {
    /// Blocks on screen, in order.
    pub strips_now: Vec<StripKey>,
    /// The rest of the boxes' blocks.
    pub strips_later: Vec<StripKey>,
    pub peaks: Vec<FileKey>,
    pub images: Vec<FileKey>,
}

impl Wants {
    pub fn is_empty(&self) -> bool {
        self.strips_now.is_empty() && self.strips_later.is_empty() && self.peaks.is_empty() && self.images.is_empty()
    }
}

/// The cache, if anything was ever loaded.
pub fn cache(cx: &App) -> Option<&MediaCache> {
    cx.try_global::<MediaCache>()
}

/// Start loading what `wants` lists and nobody asked for yet. Leaves the cache alone
/// when there's nothing new: changing it re-renders every timeline.
pub fn request(wants: Wants, cx: &mut App) {
    let fresh = |cx: &App| -> Wants {
        let cache = cx.try_global::<MediaCache>();
        let strip = |k: &StripKey| wanted(cache.and_then(|c| c.strips.get(k)));
        let mut seen = Vec::new();
        let mut keep = |k: &StripKey| {
            let new = strip(k) && !seen.contains(k);
            if new {
                seen.push(k.clone());
            }
            new
        };
        let strips_now: Vec<StripKey> = wants.strips_now.iter().filter(|k| keep(k)).cloned().collect();
        let strips_later: Vec<StripKey> = wants.strips_later.iter().filter(|k| keep(k)).cloned().collect();
        let mut peaks: Vec<FileKey> = wants.peaks.iter().filter(|k| wanted(cache.and_then(|c| c.peaks.get(*k)))).cloned().collect();
        peaks.dedup();
        let mut images: Vec<FileKey> = wants.images.iter().filter(|k| wanted(cache.and_then(|c| c.images.get(*k)))).cloned().collect();
        images.dedup();
        Wants { strips_now, strips_later, peaks, images }
    };
    let new = fresh(cx);
    if new.is_empty() {
        return;
    }
    let _ = cx.default_global::<MediaCache>();
    let Wants { strips_now, strips_later, peaks, images } = new;
    cx.update_global(|cache: &mut MediaCache, _| {
        for key in strips_now.iter().rev() {
            cache.queue.retain(|k| k != key);
            cache.queue.push_front(key.clone());
            cache.strips.insert(key.clone(), Load::Pending);
        }
        for key in strips_later {
            cache.queue.push_back(key.clone());
            cache.strips.insert(key, Load::Pending);
        }
        for key in &peaks {
            cache.peaks.insert(key.clone(), Load::Pending);
        }
        for key in &images {
            cache.images.insert(key.clone(), Load::Pending);
        }
    });
    pump(cx);
    for key in peaks {
        load_peaks(key, cx);
    }
    for key in images {
        load_image(key, cx);
    }
}

/// Load the next queued block, unless one is loading.
fn pump(cx: &mut App) {
    let next = cx.update_global(|cache: &mut MediaCache, _| {
        if cache.busy {
            return None;
        }
        let key = cache.queue.pop_front()?;
        cache.busy = true;
        Some(key)
    });
    let Some(key) = next else { return };
    let core = state::core(cx);
    let (dir, file, block) = key.clone();
    let strip = state::run(cx, async move { core.project_filmstrip(&dir, file.as_deref(), block).await });
    cx.spawn(async move |cx| {
        let frames = match strip.await {
            Ok(path) => cx.background_executor().spawn(async move { split_strip(&path) }).await,
            Err(message) => Err(message),
        };
        if let Err(message) = &frames {
            // The tiles stay blank; say why for whoever looks.
            eprintln!("Timeline thumbnails failed: {message}");
        }
        cx.update(|cx| {
            cx.update_global(|cache: &mut MediaCache, _| {
                cache.busy = false;
                let load = frames.map_or_else(|_| Load::Failed(Instant::now()), |f| Load::Ready(Arc::new(f)));
                cache.strips.insert(key, load);
            });
            pump(cx);
        });
    })
    .detach();
}

fn load_peaks(key: FileKey, cx: &mut App) {
    let core = state::core(cx);
    let (dir, file) = key.clone();
    let peaks = state::run(cx, async move { core.project_waveform(&dir, file.as_deref()).await });
    cx.spawn(async move |cx| {
        let load = match peaks.await {
            Ok(peaks) => Load::Ready(Arc::new(peaks)),
            Err(_) => Load::Failed(Instant::now()),
        };
        cx.update(|cx| cx.update_global(|cache: &mut MediaCache, _| cache.peaks.insert(key, load)));
    })
    .detach();
}

/// An added still image, shown repeated across its piece.
fn load_image(key: FileKey, cx: &mut App) {
    let path = PathBuf::from(&key.0).join(key.1.as_deref().unwrap_or_default());
    let decode = cx.background_executor().spawn(async move { decode_image(&path) });
    cx.spawn(async move |cx| {
        let load = match decode.await {
            Ok(image) => Load::Ready(Arc::new(image)),
            Err(_) => Load::Failed(Instant::now()),
        };
        cx.update(|cx| cx.update_global(|cache: &mut MediaCache, _| cache.images.insert(key, load)));
    })
    .detach();
}

/// GPUI draws BGRA.
fn render_image(mut rgba: image::RgbaImage) -> RenderImage {
    for px in rgba.pixels_mut() {
        px.0.swap(0, 2);
    }
    RenderImage::new(vec![image::Frame::new(rgba)])
}

/// A block's image (FILMSTRIP_BLOCK_S frames side by side) as one image a frame: the
/// whole strip is wider than a GPU texture may be.
fn split_strip(path: &Path) -> Result<Vec<Arc<RenderImage>>, String> {
    let strip = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
    let count = FILMSTRIP_BLOCK_S;
    let width = strip.width() / count;
    if width == 0 || strip.height() == 0 {
        return Err("The thumbnail strip is empty".into());
    }
    Ok((0..count)
        .map(|i| {
            let frame = image::imageops::crop_imm(&strip, i * width, 0, width, strip.height()).to_image();
            Arc::new(render_image(frame))
        })
        .collect())
}

fn decode_image(path: &Path) -> Result<RenderImage, String> {
    let image = image::open(path).map_err(|e| e.to_string())?;
    let image = if image.height() > IMAGE_HEIGHT {
        image.thumbnail(u32::MAX, IMAGE_HEIGHT)
    } else {
        image
    };
    Ok(render_image(image.to_rgba8()))
}
