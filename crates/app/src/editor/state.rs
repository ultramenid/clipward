//! The editor's shared state (EditorPage.tsx's hooks and use-editor-store.ts): the
//! project with undo history, the selection, timeline zoom, and every edit the
//! player, timeline and inspector make.
//!
//! The playhead lives in its own small entity (`Playhead`): it changes every frame
//! while playing, and only the player, the clock and the playhead line follow it,
//! so the tracks re-render on edits only.
use crate::state;
use clipward_core::clip_engine::TranscriptSegment;
use clipward_core::clip_engine::render::project::{
    AudioItem, MediaFile, OverlayBox, OverlayPreview, PipItem, Project, TextLayer, VideoItem,
};
use clipward_core::edit_history::EditHistory;
use clipward_core::editor::EditorProject;
use clipward_core::overlays::{self, LogoAspects, OverlayKey, OverlayView};
use clipward_core::timeline;
use gpui_kit::*;
use std::path::PathBuf;
use std::time::Duration;

const AUTOSAVE_DELAY: Duration = Duration::from_millis(600);
const OVERLAY_PREVIEW_DELAY: Duration = Duration::from_millis(300);
/// Main track height, CSS px: dragged by the timeline's top edge, kept per device.
pub const TRACK_MIN: f32 = 64.;
pub const TRACK_MAX: f32 = 240.;
pub const TRACK_DEFAULT: f32 = 120.;
/// A new text layer's length and size (canvas px).
pub const NEW_TEXT_MS: i64 = 3000;
pub const NEW_TEXT_SIZE: i64 = 72;
/// Imported music starts a little quieter and ducks under speech.
pub const NEW_SOUND_VOLUME: f64 = 0.8;
/// How long an added image shows at first.
pub const NEW_IMAGE_MS: i64 = 3000;
/// Transition lengths offered between pieces.
pub const TRANSITIONS: [i64; 4] = [0, 400, 800, 1200];
/// Title and banner size range against their automatic size.
pub const OVERLAY_SCALE_MIN: f64 = 0.25;
pub const OVERLAY_SCALE_MAX: f64 = 3.0;
/// Timeline zoom, px per second.
pub const ZOOM_MIN: f64 = 8.;
pub const ZOOM_MAX: f64 = 400.;

/// What the inspector shows: one piece, text layer, sound, overlay or PiP at a time.
#[derive(Clone, Debug, PartialEq)]
pub enum Selection {
    Piece(usize),
    Text(String),
    Sound(String),
    Overlay(OverlayKey),
    Pip(String),
}

/// The playhead (use-editor-store.ts `usePlayhead`), ms of edited time.
pub struct Playhead {
    pub time: f64,
    pub playing: bool,
    /// The time under the mouse on the timeline; the preview shows it (not while playing).
    pub hover: Option<f64>,
    /// A drag is in progress: show the instant preview and hold off engine renders.
    pub scrubbing: bool,
}

impl Playhead {
    /// The time the preview shows: the hovered one while paused, else the playhead.
    pub fn shown(&self) -> f64 {
        if self.playing { self.time } else { self.hover.unwrap_or(self.time) }
    }
}

/// What `project_load` says about the clip besides its edit.
#[derive(Clone, Debug)]
pub struct Opened {
    /// The source is still on disk; without it nothing can be edited.
    pub source_available: bool,
    /// The source video, for the instant (approximate) preview.
    pub source_path: Option<PathBuf>,
    pub source_duration_ms: i64,
    /// The campaign brief.
    pub requirements: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveState {
    Saved,
    Saving,
    Failed,
}

pub struct EditorState {
    pub output_dir: String,
    pub clip_index: u32,
    pub title: SharedString,
    pub history: EditHistory,
    /// The opened clip's facts besides the project.
    pub opened: Option<Opened>,
    pub load_error: Option<String>,
    pub playhead: Entity<Playhead>,
    pub selection: Option<Selection>,
    /// Timeline zoom, px per second; None until it fits the clip on first show.
    pub zoom: Option<f64>,
    pub track_height: f32,
    pub save: SaveState,
    pub save_error: Option<String>,
    pub notice: Option<String>,
    /// Saved edits the library's clip doesn't have yet.
    pub unexported: bool,
    /// A save or export this session: the clip list should re-read the run.
    pub changed: bool,
    pub exporting: Option<f64>,
    /// Opening: chunks rendered of the whole edit, until it plays exact (None once
    /// done or skipped).
    pub warmup: Option<(usize, usize)>,
    /// A voiceover is being recorded from this time (ms): the preview stays silent.
    pub recording: Option<i64>,
    /// Title and banner images drawn by the engine, and each logo's shape.
    previews: Vec<OverlayPreview>,
    aspects: LogoAspects,
    pub overlays: Vec<OverlayView>,
    autosave: Option<Task<()>>,
    preview_task: Option<Task<()>>,
    overlay_key: String,
}

impl EditorState {
    pub fn new(output_dir: String, clip_index: u32, title: SharedString, cx: &mut Context<Self>) -> Self {
        let playhead = cx.new(|_| Playhead { time: 0., playing: false, hover: None, scrubbing: false });
        let mut this = Self {
            output_dir,
            clip_index,
            title,
            history: EditHistory::default(),
            opened: None,
            load_error: None,
            playhead,
            selection: None,
            zoom: None,
            track_height: load_track_height(cx),
            save: SaveState::Saved,
            save_error: None,
            notice: None,
            unexported: false,
            changed: false,
            exporting: None,
            warmup: Some((0, 0)),
            recording: None,
            previews: Vec::new(),
            aspects: LogoAspects::new(),
            overlays: Vec::new(),
            autosave: None,
            preview_task: None,
            overlay_key: String::new(),
        };
        this.load(cx);
        this
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let core = state::core(cx);
        let (dir, index) = (self.output_dir.clone(), self.clip_index);
        let load = state::run(cx, async move { core.project_load(dir, index).await });
        cx.spawn(async move |this, cx| {
            let result = load.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(opened) => {
                        let EditorProject {
                            project,
                            source_available,
                            source_path,
                            source_duration_ms,
                            requirements,
                            unexported,
                        } = opened;
                        this.unexported = unexported;
                        this.history.open(project);
                        this.opened = Some(Opened { source_available, source_path, source_duration_ms, requirements });
                        this.refresh_overlays(cx);
                    }
                    Err(message) => this.load_error = Some(message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn project(&self) -> Option<&Project> {
        self.history.project()
    }

    /// The clip can be edited: its source video is still there.
    pub fn editable(&self) -> bool {
        self.opened.as_ref().is_some_and(|o| o.source_available)
    }

    pub fn total(&self) -> f64 {
        self.project().map_or(0., |p| timeline::total_length(&p.video) as f64)
    }

    pub fn run_file(&self, name: &str) -> PathBuf {
        PathBuf::from(&self.output_dir).join(name)
    }

    pub fn time(&self, cx: &App) -> f64 {
        self.playhead.read(cx).time
    }

    // ---- edits ------------------------------------------------------------------

    /// Apply an edit; `update` returns None when nothing changed. `record: false`
    /// continues the current drag's undo step.
    pub fn change(&mut self, update: impl FnOnce(&Project) -> Option<Project>, record: bool, cx: &mut Context<Self>) {
        let before = self.history.revision();
        self.history.change(update, record);
        if self.history.revision() != before {
            self.after_edit(cx);
        }
    }

    pub fn checkpoint(&mut self, cx: &mut Context<Self>) {
        self.history.checkpoint();
        cx.notify();
    }

    /// One undo step for a run of changes (a slider drag).
    pub fn begin_batch(&mut self) {
        self.history.begin_batch();
    }

    pub fn end_batch(&mut self) {
        self.history.end_batch();
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        if self.editable() && !self.history.past().is_empty() {
            self.history.undo();
            self.after_edit(cx);
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        if self.editable() && !self.history.future().is_empty() {
            self.history.redo();
            self.after_edit(cx);
        }
    }

    /// The main track, kept valid: transitions fit their pieces, keyframes their length.
    pub fn edit_video(&mut self, update: impl FnOnce(&[VideoItem]) -> Vec<VideoItem>, record: bool, cx: &mut Context<Self>) {
        self.change(
            |p| {
                let video = timeline::fit_transitions(&timeline::fit_keyframes(&update(&p.video)));
                (video != p.video).then(|| Project { video, ..p.clone() })
            },
            record,
            cx,
        );
    }

    pub fn edit_texts(&mut self, update: impl FnOnce(&[TextLayer]) -> Vec<TextLayer>, record: bool, cx: &mut Context<Self>) {
        self.change(|p| changed(&p.texts, update(&p.texts)).map(|texts| Project { texts, ..p.clone() }), record, cx);
    }

    pub fn edit_sounds(&mut self, update: impl FnOnce(&[AudioItem]) -> Vec<AudioItem>, record: bool, cx: &mut Context<Self>) {
        self.change(|p| changed(&p.audio, update(&p.audio)).map(|audio| Project { audio, ..p.clone() }), record, cx);
    }

    pub fn edit_pip(&mut self, update: impl FnOnce(&[PipItem]) -> Vec<PipItem>, record: bool, cx: &mut Context<Self>) {
        self.change(|p| changed(&p.pip, update(&p.pip)).map(|pip| Project { pip, ..p.clone() }), record, cx);
    }

    /// Caption words; never its own undo step boundary (as the TS `editTranscript`).
    pub fn edit_transcript(&mut self, update: impl FnOnce(&[TranscriptSegment]) -> Vec<TranscriptSegment>, cx: &mut Context<Self>) {
        self.change(
            |p| changed(&p.transcript, update(&p.transcript)).map(|transcript| Project { transcript, ..p.clone() }),
            false,
            cx,
        );
    }

    /// Give an overlay its own box (from where it is now) with `patch` applied.
    pub fn place_overlay(&mut self, key: OverlayKey, patch: impl FnOnce(&mut OverlayBox), record: bool, cx: &mut Context<Self>) {
        let Some(view) = self.overlays.iter().find(|o| o.key == key) else { return };
        let mut box_ = view.box_.clone();
        patch(&mut box_);
        self.change(|p| Some(overlays::with_overlay_box(p, key, Some(box_))), record, cx);
    }

    pub fn set_overlay_box(&mut self, key: OverlayKey, box_: Option<OverlayBox>, record: bool, cx: &mut Context<Self>) {
        self.change(|p| Some(overlays::with_overlay_box(p, key, box_)), record, cx);
    }

    /// Resize from a corner drag: a logo's width, or the title's or banner's scale.
    pub fn resize_overlay(&mut self, key: OverlayKey, factor: f64, base: f64, cx: &mut Context<Self>) {
        use clipward_core::clip_engine::render::service::{MAX_LOGO_SIZE, MIN_LOGO_SIZE};
        if let Some(logo) = key.logo_index() {
            let size = (timeline::js_round((base * factor).clamp(MIN_LOGO_SIZE, MAX_LOGO_SIZE) * 1000.) / 1000.).max(MIN_LOGO_SIZE);
            self.change(
                |p| {
                    let mut logos = p.logos.clone();
                    logos.get_mut(logo)?.size = size;
                    (logos != p.logos).then(|| Project { logos, ..p.clone() })
                },
                false,
                cx,
            );
        } else {
            let scale = timeline::js_round((base * factor).clamp(OVERLAY_SCALE_MIN, OVERLAY_SCALE_MAX) * 100.) / 100.;
            self.place_overlay(key, |b| b.scale = scale, false, cx);
        }
    }

    // ---- selection & playhead -----------------------------------------------------

    pub fn select(&mut self, selection: Option<Selection>, cx: &mut Context<Self>) {
        if self.selection != selection {
            self.selection = selection;
            cx.notify();
        }
    }

    pub fn seek(&mut self, t: f64, cx: &mut Context<Self>) {
        let total = self.total();
        self.playhead.update(cx, |p, cx| {
            p.time = t.clamp(0., total);
            cx.notify();
        });
    }

    /// Seek and stop (clicks on the ruler, words, the keyboard).
    pub fn seek_paused(&mut self, t: f64, cx: &mut Context<Self>) {
        self.set_playing(false, cx);
        self.seek(t, cx);
    }

    pub fn set_playing(&mut self, playing: bool, cx: &mut Context<Self>) {
        self.playhead.update(cx, |p, cx| {
            if p.playing != playing {
                p.playing = playing;
                cx.notify();
            }
        });
    }

    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        let (playing, time) = {
            let p = self.playhead.read(cx);
            (p.playing, p.time)
        };
        if !playing && time >= self.total() {
            self.seek(0., cx);
        }
        self.set_playing(!playing, cx);
    }

    pub fn set_hover(&mut self, hover: Option<f64>, cx: &mut Context<Self>) {
        self.playhead.update(cx, |p, cx| {
            if p.hover != hover {
                p.hover = hover;
                cx.notify();
            }
        });
    }

    pub fn set_scrubbing(&mut self, scrubbing: bool, cx: &mut Context<Self>) {
        self.playhead.update(cx, |p, cx| {
            if p.scrubbing != scrubbing {
                p.scrubbing = scrubbing;
                cx.notify();
            }
        });
    }

    pub fn set_zoom(&mut self, zoom: f64, cx: &mut Context<Self>) {
        self.zoom = Some(zoom.clamp(ZOOM_MIN, ZOOM_MAX));
        cx.notify();
    }

    pub fn set_track_height(&mut self, height: f32, cx: &mut Context<Self>) {
        let height = height.round().clamp(TRACK_MIN, TRACK_MAX);
        if height != self.track_height {
            self.track_height = height;
            save_track_height(height, cx);
            cx.notify();
        }
    }

    // ---- tools (EditorPage.tsx callbacks) --------------------------------------------

    /// Split the piece under the playhead (S).
    pub fn split(&mut self, cx: &mut Context<Self>) {
        let Some(fps) = self.project().map(|p| p.fps.clone()) else { return };
        let at = timeline::snap_to_frame(self.time(cx), &fps);
        self.edit_video(|v| timeline::split_at(v, at), true, cx);
    }

    /// Add a text layer at the playhead (T).
    pub fn add_text(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project() else { return };
        let total = self.total() as i64;
        let now = self.time(cx) as i64;
        let start = now.min(total - NEW_TEXT_MS.min(total)).max(0);
        let end = (start + NEW_TEXT_MS).min(total);
        if end - start < timeline::MIN_TEXT_MS {
            return;
        }
        let id = timeline::next_text_id(&project.texts);
        let layer = TextLayer {
            id: id.clone(),
            text: "Your text".into(),
            start_ms: start,
            end_ms: end,
            x: 0.5,
            y: 0.3,
            size: NEW_TEXT_SIZE,
            preset: project.caption_preset.clone(),
            animation_in: "none".into(),
            animation_out: "none".into(),
        };
        self.edit_texts(|texts| [texts, &[layer]].concat(), true, cx);
        self.select(Some(Selection::Text(id)), cx);
        self.set_playing(false, cx);
    }

    /// Delete the selection, or the piece under the playhead (Delete).
    pub fn remove_selected(&mut self, cx: &mut Context<Self>) {
        match self.selection.clone() {
            Some(Selection::Overlay(key)) => {
                self.change(|p| Some(overlays::without_overlay(p, key)), true, cx);
                self.select(None, cx);
            }
            Some(Selection::Pip(id)) => {
                self.edit_pip(|pip| pip.iter().filter(|p| p.id != id).cloned().collect(), true, cx);
                self.select(None, cx);
            }
            Some(Selection::Sound(id)) => {
                self.edit_sounds(|audio| audio.iter().filter(|a| a.id != id).cloned().collect(), true, cx);
                self.select(None, cx);
            }
            Some(Selection::Text(id)) => {
                self.edit_texts(|texts| texts.iter().filter(|t| t.id != id).cloned().collect(), true, cx);
                self.select(None, cx);
            }
            Some(Selection::Piece(index)) => {
                self.edit_video(|v| timeline::remove_item(v, index), true, cx);
                self.select(None, cx);
            }
            None => {
                let time = self.time(cx);
                let Some(index) = self.project().and_then(|p| timeline::placement_at(&p.video, time)).map(|p| p.index)
                else {
                    return;
                };
                self.edit_video(|v| timeline::remove_item(v, index), true, cx);
            }
        }
    }

    /// Whether Delete has something to delete (the last piece always stays).
    pub fn can_remove(&self) -> bool {
        let pieces = self.project().map_or(0, |p| p.video.len());
        !matches!(self.selection, Some(Selection::Piece(_)) | None) || pieces > 1
    }

    /// Move a piece's sound to the sound track, to move or trim it on its own.
    pub fn detach(&mut self, index: usize, cx: &mut Context<Self>) {
        let mut detached = None;
        self.change(
            |p| {
                let (video, audio) = timeline::detach_audio(&p.video, &p.audio, index);
                if audio == p.audio {
                    return None;
                }
                detached = audio.last().map(|a| a.id.clone());
                Some(Project { video, audio, ..p.clone() })
            },
            true,
            cx,
        );
        if let Some(id) = detached {
            self.select(Some(Selection::Sound(id)), cx);
        }
    }

    /// Add an imported video or image: into the main track at the playhead, or over
    /// the video as picture-in-picture.
    pub fn add_media(&mut self, file: MediaFile, as_pip: bool, cx: &mut Context<Self>) {
        let Some(fps) = self.project().map(|p| p.fps.clone()) else { return };
        // An image starts as a few seconds of still; a video as all of it.
        let len = if file.image { NEW_IMAGE_MS.min(file.duration_ms) } else { file.duration_ms };
        let at = timeline::snap_to_frame(self.time(cx), &fps);
        if !as_pip {
            let mut index = 0;
            self.change(
                |p| {
                    let piece = VideoItem {
                        src_in_ms: 0,
                        src_out_ms: len,
                        framing: "crop".into(),
                        crop: Some(clipward_core::clip_engine::render::project::Crop { x: 0.5, y: 0.5, zoom: 1.0 }),
                        media: Some(file.file.clone()),
                        volume: 1.0,
                        effect: "fade".into(),
                        ..VideoItem::default()
                    };
                    let (video, at_index) = timeline::insert_at(&p.video, at, piece);
                    index = at_index;
                    let mut media = p.media.clone();
                    media.push(file.clone());
                    Some(Project { media, video: timeline::fit_transitions(&video), ..p.clone() })
                },
                true,
                cx,
            );
            self.select(Some(Selection::Piece(index)), cx);
        } else {
            let mut id = String::new();
            self.change(
                |p| {
                    id = timeline::next_pip_id(&p.pip);
                    let room = (timeline::total_length(&p.video) - at).max(timeline::MIN_ITEM_MS);
                    let item = PipItem {
                        id: id.clone(),
                        media: file.file.clone(),
                        src_in_ms: 0,
                        src_out_ms: len.min(room),
                        start_ms: at,
                        x: 0.5,
                        y: 0.35,
                        width: 0.5,
                        volume: if file.image { 0.0 } else { 1.0 },
                    };
                    let mut media = p.media.clone();
                    media.push(file.clone());
                    let mut pip = p.pip.clone();
                    pip.push(item);
                    Some(Project { media, pip, ..p.clone() })
                },
                true,
                cx,
            );
            self.select(Some(Selection::Pip(id)), cx);
        }
    }

    /// Add imported music or a sound effect at the playhead.
    pub fn add_sound(&mut self, imported: clipward_core::ImportedAudio, cx: &mut Context<Self>) {
        let Some(project) = self.project() else { return };
        let total = self.total() as i64;
        let start = (self.time(cx) as i64).min((total - timeline::MIN_ITEM_MS).max(0));
        let len = imported.duration_ms.min(total - start).max(timeline::MIN_ITEM_MS);
        let id = timeline::next_sound_id(&project.audio);
        let item = AudioItem {
            id: id.clone(),
            name: imported.name,
            file: Some(imported.file),
            src_in_ms: 0,
            src_out_ms: len,
            start_ms: start,
            volume: NEW_SOUND_VOLUME,
            fade_in_ms: 0,
            fade_out_ms: 1000.min(len),
            duck: true,
            voice: false,
            speed: 1.0,
        };
        self.edit_sounds(|audio| [audio, &[item]].concat(), true, cx);
        self.select(Some(Selection::Sound(id)), cx);
    }

    /// A recorded voiceover from `start`: recording over an earlier take replaces
    /// that part of it.
    pub fn add_voiceover(&mut self, take: clipward_core::ImportedAudio, start: i64, cx: &mut Context<Self>) {
        let total = self.total() as i64;
        let len = take.duration_ms.min(total - start).max(timeline::MIN_ITEM_MS);
        let mut id = String::new();
        self.change(
            |p| {
                id = timeline::next_sound_id(&p.audio);
                let item = AudioItem {
                    id: id.clone(),
                    name: take.name.clone(),
                    file: Some(take.file.clone()),
                    src_in_ms: 0,
                    src_out_ms: len,
                    start_ms: start,
                    volume: 1.0,
                    fade_in_ms: 0,
                    fade_out_ms: 0,
                    duck: false,
                    voice: true,
                    speed: 1.0,
                };
                Some(Project { audio: timeline::punch_in(&p.audio, item), ..p.clone() })
            },
            true,
            cx,
        );
        self.select(Some(Selection::Sound(id)), cx);
    }

    /// Add a logo the user imported into the run (top right, 15% wide).
    pub fn add_logo(&mut self, name: String, cx: &mut Context<Self>) {
        let mut index = 0;
        self.change(
            |p| {
                index = p.logos.len();
                let mut logos = p.logos.clone();
                logos.push(clipward_core::clip_engine::render::project::LogoLayer {
                    path: name.clone(),
                    position: "top-right".into(),
                    size: 0.15,
                    placed: None,
                });
                Some(Project { logos, ..p.clone() })
            },
            true,
            cx,
        );
        self.select(Some(Selection::Overlay(OverlayKey::Logo(index))), cx);
    }

    pub fn add_title(&mut self, cx: &mut Context<Self>) {
        self.change(|p| Some(Project { title: Some("Your title".into()), ..p.clone() }), true, cx);
        self.select(Some(Selection::Overlay(OverlayKey::Title)), cx);
    }

    /// Save the edit and render it over the clip in the library.
    pub fn export(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project().cloned() else { return };
        if self.exporting.is_some() || !self.editable() {
            return;
        }
        self.set_playing(false, cx);
        self.exporting = Some(0.);
        self.notice = None;
        self.save_error = None;
        cx.notify();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<f64>();
        let core = state::core(cx);
        let (dir, index) = (self.output_dir.clone(), self.clip_index);
        let export = state::run(cx, async move {
            core.project_export(&dir, index, &project, move |p| {
                let _ = progress_tx.send(p);
            })
            .await
        });
        cx.spawn(async move |this, cx| {
            let progress = this.clone();
            let mut watch = cx.spawn(async move |cx| {
                while let Some(p) = progress_rx.recv().await {
                    let _ = progress.update(cx, |this, cx| {
                        this.exporting = Some(p);
                        cx.notify();
                    });
                }
            });
            let result = export.await;
            let _ = this.update(cx, |this, cx| {
                this.exporting = None;
                match result {
                    Ok(()) => {
                        this.changed = true;
                        this.unexported = false;
                        this.save = SaveState::Saved;
                        this.notice = Some("Exported. The clip in your library now has these edits.".into());
                    }
                    Err(message) => {
                        this.save_error = Some(if message.is_empty() {
                            "Export failed. Your original clip is unchanged.".into()
                        } else {
                            message
                        });
                    }
                }
                cx.notify();
            });
            // The sender is gone once the export returns; the watcher ends with it.
            (&mut watch).await;
        })
        .detach();
    }

    // ---- after every edit -----------------------------------------------------------

    fn after_edit(&mut self, cx: &mut Context<Self>) {
        self.keep_valid(cx);
        self.refresh_overlays(cx);
        self.schedule_autosave(cx);
        cx.notify();
    }

    /// Keep the playhead and selection valid after edits shorten or remove things.
    fn keep_valid(&mut self, cx: &mut Context<Self>) {
        let total = self.total();
        if self.time(cx) > total {
            self.seek(total, cx);
        }
        let Some(p) = self.project() else { return };
        let gone = match &self.selection {
            Some(Selection::Piece(i)) => *i >= p.video.len(),
            Some(Selection::Text(id)) => !p.texts.iter().any(|t| &t.id == id),
            Some(Selection::Sound(id)) => !p.audio.iter().any(|a| &a.id == id),
            Some(Selection::Pip(id)) => !p.pip.iter().any(|x| &x.id == id),
            // The title keeps its inspector while its text is retyped (an empty title draws nothing).
            Some(Selection::Overlay(OverlayKey::Title)) => p.title.is_none(),
            Some(Selection::Overlay(key)) => !self.overlays.iter().any(|o| &o.key == key),
            None => false,
        };
        if gone {
            self.selection = None;
        }
    }

    /// Autosave each edit once the user pauses.
    fn schedule_autosave(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project().cloned() else { return };
        self.save = SaveState::Saving;
        let core = state::core(cx);
        let (dir, index) = (self.output_dir.clone(), self.clip_index);
        self.autosave = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE_DELAY).await;
            let Ok(save) = this.update(cx, |_, cx| {
                state::run(cx, async move { core.project_save(dir, index, project).await })
            }) else {
                return;
            };
            let result = save.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(lacking) => {
                        this.changed = true;
                        this.unexported = lacking;
                        this.save = SaveState::Saved;
                        this.save_error = None;
                    }
                    Err(message) => {
                        this.save = SaveState::Failed;
                        this.save_error = Some(message);
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Recompute the overlay layers; ask the engine for new title/banner images when
    /// their text or the canvas changed, and read new logos' shapes.
    fn refresh_overlays(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project().cloned() else { return };
        let key = serde_json::to_string(&(&project.title, &project.banner_platform, &project.banner_channel_url, project.width))
            .unwrap_or_default();
        if key != self.overlay_key {
            self.overlay_key = key;
            let core = state::core(cx);
            let dir = self.output_dir.clone();
            let at = self.time(cx) as i64;
            let p = project.clone();
            self.preview_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(OVERLAY_PREVIEW_DELAY).await;
                let previews = cx
                    .update(|_| core.project_overlay_previews(&dir, &p, at))
                    .unwrap_or_default();
                let _ = this.update(cx, |this, cx| {
                    this.previews = previews;
                    this.rebuild_overlays();
                    cx.notify();
                });
            }));
        }
        for logo in &project.logos {
            if self.aspects.contains_key(&logo.path) {
                continue;
            }
            let path = self.run_file(&logo.path);
            if let Ok((w, h)) = image::image_dimensions(&path) {
                if h > 0 {
                    self.aspects.insert(logo.path.clone(), w as f64 / h as f64);
                }
            }
        }
        self.rebuild_overlays();
    }

    fn rebuild_overlays(&mut self) {
        let total = self.total() as i64;
        self.overlays = match self.history.project() {
            Some(p) => overlays::overlay_views(p, &self.previews, &self.aspects, total, &self.output_dir),
            None => Vec::new(),
        };
    }

    /// Media the project imported (videos and images added to the main track or as PiP).
    pub fn media_file(&self, name: &str) -> Option<&MediaFile> {
        self.project()?.media.iter().find(|m| m.file == name)
    }
}

/// The new list, or None when it equals the old one.
fn changed<T: PartialEq>(old: &[T], new: Vec<T>) -> Option<Vec<T>> {
    (new.as_slice() != old).then_some(new)
}

fn track_file(cx: &App) -> PathBuf {
    state::core(cx).paths.user_data.join("editor.json")
}

/// The timeline height is kept per device (React kept it in localStorage).
fn load_track_height(cx: &App) -> f32 {
    std::fs::read(track_file(cx))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|v| v["trackHeight"].as_f64())
        .map_or(TRACK_DEFAULT, |h| (h as f32).round().clamp(TRACK_MIN, TRACK_MAX))
}

fn save_track_height(height: f32, cx: &App) {
    let bytes = serde_json::to_vec(&serde_json::json!({ "trackHeight": height })).unwrap_or_default();
    // Not saved: the next open starts at the default.
    let _ = clipward_core::paths::write_atomic(&track_file(cx), &bytes);
}
