//! The clip editor (EditorPage.tsx, Player.tsx, Timeline.tsx and its panels).
//! Opened from a clip list; emits `EditorClosed` when the user goes back.
//!
//! The page holds the layout, the toolbar, the keyboard and the file pickers; the
//! edit itself lives in `EditorState`. The player, timeline and side panel are
//! their own cached views, so a frame of playback redraws only what follows the
//! playhead, not the whole page.
use crate::recorder::Recorder;
use crate::shell::{self, Filter};
use crate::state as app;
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::{self, CalloutTone, TypeScale, back_link, u};
use clipward_core::clip_engine::render::project::{AUDIO_IMPORT_EXTENSIONS, IMAGE_EXTENSIONS, VIDEO_IMPORT_EXTENSIONS};
use clipward_core::format::format_clock;
use gpui_kit::component::{input::{Redo, Undo}, tooltip::Tooltip};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use player::Player;
use state::{EditorState, Playhead, SaveState, ZOOM_MAX, ZOOM_MIN};

pub mod player;
pub mod state;

// While the port is in progress the timeline and the side panel sit behind
// features (`editor-timeline`, `editor-inspector`; on in `all-pages`), so one in
// mid-edit can't break the rest; when off, a blank stand-in keeps the layout.
#[cfg(feature = "editor-timeline")]
pub mod timeline;
#[cfg(not(feature = "editor-timeline"))]
pub mod timeline {
    use super::state::EditorState;
    use gpui_kit::*;
    pub struct Timeline;
    impl Timeline {
        pub fn new(_: Entity<EditorState>, _: &mut Window, _: &mut Context<Self>) -> Self {
            Self
        }
        pub fn height(&self, _: &App) -> Rems {
            crate::ui::u(160.)
        }
    }
    impl Render for Timeline {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().h(crate::ui::u(160.)).w_full()
        }
    }
}
#[cfg(feature = "editor-inspector")]
pub mod inspector;
#[cfg(not(feature = "editor-inspector"))]
pub mod inspector {
    use super::state::EditorState;
    use gpui_kit::*;
    pub struct Inspector;
    impl Inspector {
        pub fn new(_: Entity<EditorState>, _: &mut Window, _: &mut Context<Self>) -> Self {
            Self
        }
    }
    impl Render for Inspector {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(crate::ui::u(288.)).h_full()
        }
    }
}

/// Below this window width (CSS px) the side panel opens over the preview.
const WIDE_MIN: f32 = 1024.;
/// One frame at 30 fps: the arrow keys' step.
const FRAME_MS: f64 = 1000. / 30.;

/// The user left the editor; `changed` when the run's files may differ (saved
/// edits or an export), so the clip list should re-read the run.
pub struct EditorClosed {
    pub changed: bool,
}

/// A voiceover take: recording from `start` (ms) while the clip plays muted.
struct Take {
    recorder: Recorder,
    start: i64,
}

pub struct EditorPage {
    editor: Entity<EditorState>,
    player: Entity<Player>,
    timeline: Entity<timeline::Timeline>,
    inspector: Entity<inspector::Inspector>,
    clock: Entity<Clock>,
    take_button: Entity<TakeButton>,
    focus: FocusHandle,
    /// The side panel, over the preview in narrow windows.
    panel_open: bool,
    adding_media: bool,
    importing: bool,
    /// Asking for the microphone (the first take shows the system prompt).
    arming: bool,
    take: Option<Take>,
    /// The timeline's top edge being dragged: pointer y and height at the start.
    resizing: Option<(Pixels, f32)>,
    resize_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<EditorClosed> for EditorPage {}

impl EditorPage {
    pub fn new(output_dir: String, clip_index: u32, title: SharedString, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| EditorState::new(output_dir, clip_index, title, cx));
        let playhead = editor.read(cx).playhead.clone();
        let player = cx.new(|cx| Player::new(editor.clone(), window, cx));
        let timeline = cx.new(|cx| timeline::Timeline::new(editor.clone(), window, cx));
        let inspector = cx.new(|cx| inspector::Inspector::new(editor.clone(), window, cx));
        let clock = cx.new(|cx| {
            Clock::new(editor.clone(), cx, |p, e| {
                // While playing it ticks in tenths, the clock's resolution.
                let time = if p.playing { (p.time / 100.).floor() * 100. } else { p.time };
                format!("{} / {}", format_clock(time), format_clock(e.total()))
            })
        });
        let take_clock = cx.new(|cx| {
            Clock::new(editor.clone(), cx, |p, e| {
                let time = if p.playing { (p.time / 100.).floor() * 100. } else { p.time };
                format!("Stop {}", format_clock(time - e.recording.unwrap_or(0) as f64))
            })
        });
        let page = cx.entity().downgrade();
        let take_button = cx.new(|cx| TakeButton::new(page, take_clock, cx));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let subscriptions = vec![
            cx.observe(&editor, |_, _, cx| cx.notify()),
            // The take ends with the clip, or when the user pauses.
            cx.observe(&playhead, |this, playhead, cx| {
                if this.take.is_some() && !playhead.read(cx).playing {
                    this.stop_recording(cx);
                }
            }),
        ];
        Self {
            editor,
            player,
            timeline,
            inspector,
            clock,
            take_button,
            focus,
            panel_open: false,
            adding_media: false,
            importing: false,
            arming: false,
            take: None,
            resizing: None,
            resize_focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// The edit this page shows (the snapshot probe drives it).
    #[allow(dead_code)]
    pub fn editor(&self) -> &Entity<EditorState> {
        &self.editor
    }

    #[allow(dead_code)]
    pub fn player(&self) -> &Entity<Player> {
        &self.player
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        // Leaving mid-take drops the recorder, which releases the microphone.
        self.take = None;
        let changed = self.editor.read(cx).changed;
        cx.emit(EditorClosed { changed });
    }

    fn fail(&mut self, message: String, fallback: &str, cx: &mut Context<Self>) {
        let message = if message.trim().is_empty() { fallback.to_string() } else { message };
        self.editor.update(cx, |e, cx| {
            e.save_error = Some(message);
            cx.notify();
        });
    }

    // ---- adding things ---------------------------------------------------------------

    /// Add a video or image: into the main track at the playhead, or over the video
    /// (picture-in-picture).
    fn add_media(&mut self, as_pip: bool, cx: &mut Context<Self>) {
        let Some(fps) = self.editor.read(cx).project().map(|p| p.fps.clone()) else { return };
        self.editor.update(cx, |e, cx| e.set_playing(false, cx));
        self.adding_media = true;
        cx.notify();
        let core = app::core(cx);
        let dir = self.editor.read(cx).output_dir.clone();
        cx.spawn(async move |this, cx| {
            let kinds: Vec<&str> = VIDEO_IMPORT_EXTENSIONS.iter().chain(IMAGE_EXTENSIONS).copied().collect();
            let picked = shell::pick_file("Add a Video or Image", &[Filter("Videos and images", &kinds)]).await;
            let result = match picked {
                Some(path) => match this.update(cx, |_, cx| {
                    app::run(cx, async move { core.project_import_media(&dir, &path, &fps).await })
                }) {
                    Ok(import) => import.await.map(Some),
                    Err(_) => return,
                },
                None => Ok(None),
            };
            let _ = this.update(cx, |this, cx| {
                this.adding_media = false;
                match result {
                    Ok(Some(file)) => this.editor.update(cx, |e, cx| e.add_media(file, as_pip, cx)),
                    Ok(None) => {}
                    Err(message) => this.fail(message, "Could not add that video or image.", cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_sound(&mut self, cx: &mut Context<Self>) {
        if self.editor.read(cx).project().is_none() {
            return;
        }
        self.editor.update(cx, |e, cx| e.set_playing(false, cx));
        self.importing = true;
        cx.notify();
        let core = app::core(cx);
        let dir = self.editor.read(cx).output_dir.clone();
        cx.spawn(async move |this, cx| {
            let picked = shell::pick_file("Add Music or a Sound", &[Filter("Audio", AUDIO_IMPORT_EXTENSIONS)]).await;
            let result = match picked {
                Some(path) => match this.update(cx, |_, cx| {
                    app::run(cx, async move { core.project_import_audio(&dir, &path).await })
                }) {
                    Ok(import) => import.await.map(Some),
                    Err(_) => return,
                },
                None => Ok(None),
            };
            let _ = this.update(cx, |this, cx| {
                this.importing = false;
                match result {
                    Ok(Some(sound)) => this.editor.update(cx, |e, cx| e.add_sound(sound, cx)),
                    Ok(None) => {}
                    Err(message) => this.fail(message, "Could not add that sound.", cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- voiceover -----------------------------------------------------------------

    /// Record from the playhead while the clip plays muted.
    fn start_recording(&mut self, cx: &mut Context<Self>) {
        if self.take.is_some() || self.arming || self.editor.read(cx).project().is_none() {
            return;
        }
        self.arming = true;
        let path = std::env::temp_dir().join(format!("clipward-voiceover-{}.m4a", std::process::id()));
        cx.spawn(async move |this, cx| {
            let started = Recorder::start(path).await;
            let _ = this.update(cx, |this, cx| {
                this.arming = false;
                let recorder = match started {
                    Ok(recorder) => recorder,
                    Err(message) => {
                        return this.fail(
                            message,
                            "Could not use the microphone. Allow Clipward to use it in System Settings → Privacy & Security → Microphone.",
                            cx,
                        );
                    }
                };
                let editor = this.editor.clone();
                let start = editor.update(cx, |e, cx| {
                    let (now, total) = (e.time(cx), e.total());
                    let start = if now >= total { 0. } else { now };
                    e.seek(start, cx);
                    e.recording = Some(start as i64);
                    start as i64
                });
                this.take = Some(Take { recorder, start });
                editor.update(cx, |e, cx| e.set_playing(true, cx));
                cx.notify();
            });
        })
        .detach();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        let Some(Take { recorder, start }) = self.take.take() else { return };
        self.editor.update(cx, |e, cx| {
            e.recording = None;
            e.set_playing(false, cx);
            cx.notify();
        });
        cx.notify();
        let file = match recorder.stop() {
            Ok(file) => file,
            Err(message) => return self.fail(message, "Could not save the voiceover.", cx),
        };
        let core = app::core(cx);
        let dir = self.editor.read(cx).output_dir.clone();
        let import = app::run(cx, async move {
            let data = tokio::fs::read(&file).await.map_err(|e| e.to_string());
            let _ = tokio::fs::remove_file(&file).await;
            core.project_import_recording(&dir, &data?, "m4a").await
        });
        cx.spawn(async move |this, cx| {
            let result = import.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(take) => this.editor.update(cx, |e, cx| e.add_voiceover(take, start, cx)),
                Err(message) => this.fail(message, "Could not save the voiceover.", cx),
            });
        })
        .detach();
    }

    // ---- keyboard ------------------------------------------------------------------

    /// Space, ⌘Z/⇧⌘Z, S, T, Delete, ←/→ (Shift: a second). Not while typing.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Text fields and sliders keep their keys (React skipped input, textarea, select).
        if !(self.focus.is_focused(window) || window.focused(cx).is_none()) {
            return;
        }
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;
        let command = mods.platform || mods.control;
        let editable = self.editor.read(cx).editable();
        let handled = match key {
            "space" if !command => {
                self.editor.update(cx, |e, cx| e.toggle_play(cx));
                true
            }
            "z" if command => {
                if editable {
                    self.editor.update(cx, |e, cx| if mods.shift { e.redo(cx) } else { e.undo(cx) });
                }
                true
            }
            _ if !editable => false,
            "s" if !command => {
                self.editor.update(cx, |e, cx| e.split(cx));
                true
            }
            "t" if !command => {
                self.editor.update(cx, |e, cx| e.add_text(cx));
                true
            }
            "backspace" | "delete" => {
                self.editor.update(cx, |e, cx| e.remove_selected(cx));
                true
            }
            "left" | "right" => {
                let step = if mods.shift { 1000. } else { FRAME_MS };
                let step = if key == "left" { -step } else { step };
                self.editor.update(cx, |e, cx| {
                    e.set_playing(false, cx);
                    let at = e.time(cx) + step;
                    e.seek(at, cx);
                });
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }

    fn wide(window: &Window) -> bool {
        let scale = f32::from(window.rem_size()) / 16.;
        f32::from(window.viewport_size().width) / scale >= WIDE_MIN
    }

    // ---- pieces of the page ----------------------------------------------------------

    fn header(&self, window: &Window, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let e = self.editor.read(cx);
        let editable = e.editable();
        let status: Option<(SharedString, Option<&str>)> = match e.save {
            SaveState::Saving => Some(("Saving…".into(), None)),
            SaveState::Failed => Some(("Not saved".into(), None)),
            SaveState::Saved if e.unexported => Some((
                "Saved · not exported".into(),
                Some("Your edits are saved here. Export to put them into the clip in your library."),
            )),
            SaveState::Saved if e.project().is_some() && e.history.revision() > 0 => Some(("Saved".into(), None)),
            SaveState::Saved => None,
        };
        let (can_undo, can_redo) = (!e.history.past().is_empty(), !e.history.future().is_empty());
        let exporting = e.exporting;
        let title = e.title.clone();
        let wide = Self::wide(window);
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(u(8.))
            .border_b_1()
            .border_color(t.stroke)
            .px(u(16.))
            .py(u(8.))
            .child(back_link("Clips".into(), t, cx.listener(|this, _, _, cx| this.close(cx))))
            .child(
                div()
                    .id("editor-title")
                    .ml(u(4.))
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_label()
                    .text_color(t.ink)
                    .child(title.clone())
                    .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx)),
            )
            .when_some(status, |d, (text, tip)| {
                d.child(
                    div()
                        .id("save-status")
                        .text_meta()
                        .text_color(t.ink_tertiary)
                        .child(text)
                        .when_some(tip, |d, tip| d.tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))),
                )
            })
            .when(!wide, |d| {
                d.child(
                    ui::Btn::new("editor-panel")
                        .ghost()
                        .icon("panel-right")
                        .tooltip("Show the brief and inspector")
                        .variant(if self.panel_open { ui::Variant::Secondary } else { ui::Variant::Ghost })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.panel_open = !this.panel_open;
                            cx.notify();
                        })),
                )
            })
            .child(
                ui::Btn::new("editor-undo")
                    .ghost()
                    .icon("undo-2")
                    .tooltip("Undo (⌘Z)")
                    .disabled(!editable || !can_undo)
                    .on_click(cx.listener(|this, _, _, cx| this.editor.update(cx, |e, cx| e.undo(cx)))),
            )
            .child(
                ui::Btn::new("editor-redo")
                    .ghost()
                    .icon("redo-2")
                    .tooltip("Redo (⇧⌘Z)")
                    .disabled(!editable || !can_redo)
                    .on_click(cx.listener(|this, _, _, cx| this.editor.update(cx, |e, cx| e.redo(cx)))),
            )
            .child(
                ui::Btn::new("editor-export")
                    .primary()
                    .icon("download")
                    .label(match exporting {
                        Some(p) => format!("Exporting {}%", (p * 100.).round()),
                        None => "Export".into(),
                    })
                    .loading(exporting.is_some())
                    .disabled(!editable || exporting.is_some())
                    .on_click(cx.listener(|this, _, _, cx| this.editor.update(cx, |e, cx| e.export(cx)))),
            )
    }

    fn callouts(&self, t: &Tokens, cx: &mut Context<Self>) -> Option<Div> {
        let e = self.editor.read(cx);
        let missing = e.opened.as_ref().is_some_and(|o| !o.source_available);
        if e.load_error.is_none() && e.save_error.is_none() && e.notice.is_none() && !missing {
            return None;
        }
        let text = |s: &str| Some(div().child(s.to_string()).into_any_element());
        let mut list = div().flex().flex_col().flex_shrink_0().gap(u(8.)).px(u(16.)).pt(u(12.));
        if let Some(error) = &e.load_error {
            list = list.child(ui::callout(CalloutTone::Danger, None, text(error), None, None, t));
        }
        if missing {
            list = list.child(ui::callout(
                CalloutTone::Warning,
                None,
                text("This clip's source video was cleaned up or moved, so it can't be edited. Keep source videos longer in Settings → Output."),
                None,
                None,
                t,
            ));
        }
        if let Some(error) = &e.save_error {
            let dismiss = cx.listener(|this, _, _, cx| {
                this.editor.update(cx, |e, cx| {
                    e.save_error = None;
                    cx.notify();
                })
            });
            list = list.child(div().id("save-error").child(ui::callout(CalloutTone::Danger, None, text(error), None, Some(Box::new(dismiss)), t)));
        }
        if let Some(notice) = &e.notice {
            let dismiss = cx.listener(|this, _, _, cx| {
                this.editor.update(cx, |e, cx| {
                    e.notice = None;
                    cx.notify();
                })
            });
            list = list.child(div().id("notice").child(ui::callout(CalloutTone::Success, None, text(notice), None, Some(Box::new(dismiss)), t)));
        }
        Some(list)
    }

    fn toolbar(&self, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let e = self.editor.read(cx);
        let editable = e.editable();
        let loaded = e.project().is_some();
        let can_remove = e.can_remove();
        let playing = e.playhead.read(cx).playing;
        let zoom = e.zoom.unwrap_or(60.);
        let recording = self.take.is_some();
        let tool = |id: &'static str, icon: &'static str, label: &'static str, tip: &'static str| {
            ui::Btn::new(id).ghost().small().icon(icon).label(label).tooltip(tip)
        };
        div()
            .flex()
            .items_center()
            .gap(u(4.))
            .px(u(12.))
            .py(u(6.))
            .child(
                ui::Btn::new("editor-play")
                    .ghost()
                    .icon(if playing { "pause" } else { "play" })
                    .tooltip("Play or pause (Space)")
                    .disabled(!loaded)
                    .on_click(cx.listener(|this, _, _, cx| this.editor.update(cx, |e, cx| e.toggle_play(cx)))),
            )
            .child(div().min_w(u(120.)).text_meta().text_color(t.ink_secondary).child(self.clock.clone()))
            .child(
                tool("editor-split", "scissors", "Split", "Split at the playhead (S)")
                    .disabled(!editable)
                    .on_click(cx.listener(|this, _, _, cx| this.editor.update(cx, |e, cx| e.split(cx)))),
            )
            .child(
                tool("editor-text", "type", "Text", "Add text at the playhead (T)")
                    .disabled(!editable)
                    .on_click(cx.listener(|this, _, _, cx| this.editor.update(cx, |e, cx| e.add_text(cx)))),
            )
            .child(
                tool("editor-media", "film", "Media", "Add a video or image at the playhead")
                    .loading(self.adding_media)
                    .disabled(!editable || self.adding_media)
                    .on_click(cx.listener(|this, _, _, cx| this.add_media(false, cx))),
            )
            .child(
                tool("editor-overlay", "picture-in-picture-2", "Overlay", "Add a video or image over the clip (picture-in-picture)")
                    .disabled(!editable || self.adding_media)
                    .on_click(cx.listener(|this, _, _, cx| this.add_media(true, cx))),
            )
            .child(
                tool("editor-audio", "music", "Audio", "Add music or a sound effect at the playhead")
                    .loading(self.importing)
                    .disabled(!editable || self.importing)
                    .on_click(cx.listener(|this, _, _, cx| this.add_sound(cx))),
            )
            .child(if recording {
                self.take_button.clone().into_any_element()
            } else {
                tool("editor-voiceover", "mic", "Voiceover", "Record a voiceover from the playhead (the clip plays muted)")
                    .loading(self.arming)
                    .disabled(!editable || self.importing || self.arming)
                    .on_click(cx.listener(|this, _, _, cx| this.start_recording(cx)))
                    .into_any_element()
            })
            .child(
                tool("editor-delete", "trash", "Delete", "Delete the selected piece, text, sound or overlay (Delete)")
                    .disabled(!editable || !can_remove)
                    .on_click(cx.listener(|this, _, _, cx| this.editor.update(cx, |e, cx| e.remove_selected(cx)))),
            )
            .child(div().flex_1())
            .child(
                ui::Btn::new("editor-zoom-out")
                    .ghost()
                    .icon("zoom-out")
                    .tooltip("Zoom out")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.editor.update(cx, |e, cx| e.set_zoom((zoom / 1.5).max(ZOOM_MIN), cx))
                    })),
            )
            .child(
                ui::Btn::new("editor-zoom-in")
                    .ghost()
                    .icon("zoom-in")
                    .tooltip("Zoom in")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.editor.update(cx, |e, cx| e.set_zoom((zoom * 1.5).min(ZOOM_MAX), cx))
                    })),
            )
    }

    /// The timeline's top edge: drag it (or ↑/↓ while focused) to change the main
    /// track's height.
    fn resize_edge(&self, window: &Window, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let dragging = self.resizing.is_some();
        let scale = f32::from(window.rem_size()) / 16.;
        div()
            .id("timeline-height")
            .track_focus(&self.resize_focus)
            .absolute()
            .left_0()
            .right_0()
            .top(u(-4.))
            .h(u(8.))
            .cursor(CursorStyle::ResizeRow)
            .hover(|s| s.bg(t.accent.opacity(0.4)))
            .when(self.resize_focus.is_focused(window), |d| d.bg(t.accent.opacity(0.6)))
            .when(dragging, |d| d.bg(t.accent.opacity(0.4)))
            .tooltip(|window, cx| Tooltip::new("Drag to resize the timeline").build(window, cx))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, _, cx| {
                    let height = this.editor.read(cx).track_height;
                    this.resizing = Some((e.position.y, height));
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let step = match e.keystroke.key.as_str() {
                    "up" => 8.,
                    "down" => -8.,
                    _ => return,
                };
                cx.stop_propagation();
                this.editor.update(cx, |ed, cx| {
                    let height = ed.track_height + step;
                    ed.set_track_height(height, cx);
                });
            }))
            .child(
                // While dragging, follow the pointer anywhere in the window.
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        if !dragging {
                            return;
                        }
                        let moving = entity.clone();
                        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                            if phase != DispatchPhase::Bubble {
                                return;
                            }
                            moving.update(cx, |this, cx| {
                                let Some((y, height)) = this.resizing else { return };
                                // Window px to the CSS px the height is kept in.
                                let delta = f32::from(y - e.position.y) / scale;
                                this.editor.update(cx, |ed, cx| ed.set_track_height(height + delta, cx));
                            });
                        });
                        let ending = entity.clone();
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                ending.update(cx, |this, cx| {
                                    this.resizing = None;
                                    cx.notify();
                                });
                            }
                        });
                    },
                )
                .size_full(),
            )
    }
}

impl Render for EditorPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        let (loaded, load_error, editable, warmup, brief_ready) = {
            let e = self.editor.read(cx);
            (e.project().is_some(), e.load_error.is_some(), e.editable(), e.warmup, e.opened.is_some())
        };
        let wide = Self::wide(window);
        let header = self.header(window, &t, cx).into_any_element();
        let callouts = self.callouts(&t, cx);
        let toolbar = self.toolbar(&t, cx).into_any_element();
        let resize_edge = self.resize_edge(window, &t, cx).into_any_element();
        // Not cached: a cached view that redraws redraws its cached children too, so
        // the playhead line moving would rebuild every track each frame. The timeline
        // caches its tracks and lines itself.
        let timeline = self.timeline.clone();

        let preview = if loaded {
            AnyView::from(self.player.clone()).cached(StyleRefinement::default().size_full()).into_any_element()
        } else if !load_error {
            div().mx_auto().h_full().max_w(u(384.)).child(ui::skeleton(&t)).into_any_element()
        } else {
            div().into_any_element()
        };
        let inspector = AnyView::from(self.inspector.clone()).cached(StyleRefinement::default().w(u(288.)).h_full().flex_shrink_0());
        let aside = if wide {
            Some(div().flex().flex_shrink_0().h_full().child(inspector).into_any_element())
        } else if self.panel_open {
            Some(
                div()
                    .absolute()
                    .top(u(16.))
                    .bottom(u(16.))
                    .right(u(16.))
                    .flex()
                    .rounded(u(12.))
                    .bg(t.bg)
                    .p(u(8.))
                    .shadow_xl()
                    .occlude()
                    .child(inspector)
                    .into_any_element(),
            )
        } else {
            None
        };

        div()
            .id("editor")
            .track_focus(&self.focus)
            .key_context("Editor")
            .on_key_down(cx.listener(Self::on_key))
            // The Edit menu's Undo and Redo, when no text field has them.
            .on_action(cx.listener(|this, _: &Undo, _, cx| this.editor.update(cx, |e, cx| e.undo(cx))))
            .on_action(cx.listener(|this, _: &Redo, _, cx| this.editor.update(cx, |e, cx| e.redo(cx))))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .children(callouts)
            .child(
                div()
                    .relative()
                    .flex()
                    .min_h_0()
                    .flex_1()
                    .gap(u(16.))
                    .p(u(16.))
                    .child(div().min_h_0().min_w_0().flex_1().child(preview))
                    .children(aside),
            )
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(t.stroke)
                    .child(resize_edge)
                    .child(toolbar)
                    .when(loaded && brief_ready, move |d| {
                        d.child(
                            div()
                                .relative()
                                .when(!editable, |d| d.opacity(0.6))
                                .child(timeline)
                                // Read-only: the timeline shows but takes no pointer input.
                                .when(!editable, |d| d.child(div().absolute().inset_0().occlude())),
                        )
                    }),
            )
            .when_some(warmup.filter(|_| loaded), |d, (done, total)| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(t.bg.opacity(0.95))
                        .occlude()
                        .child(
                            div()
                                .w(u(288.))
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(u(12.))
                                .child(div().text_label().text_color(t.ink).child("Preparing the editor…"))
                                .child(ui::progress_bar(if total > 0 { done as f64 / total as f64 * 100. } else { 0. }, 6., &t))
                                .child(div().text_meta().text_color(t.ink_tertiary).child(if total > 0 {
                                    format!("Rendering the clip so it plays smoothly · {done} of {total}")
                                } else {
                                    "Rendering the clip so it plays smoothly".into()
                                }))
                                .child(
                                    ui::Btn::new("warmup-skip")
                                        .ghost()
                                        .small()
                                        .label("Skip")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.editor.update(cx, |e, cx| {
                                                e.warmup = None;
                                                cx.notify();
                                            })
                                        })),
                                ),
                        ),
                )
            })
    }
}

/// The Stop button while recording, its label ticking with the take.
pub struct TakeButton {
    page: WeakEntity<EditorPage>,
    clock: Entity<Clock>,
    _subscription: Subscription,
}

impl TakeButton {
    fn new(page: WeakEntity<EditorPage>, clock: Entity<Clock>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&clock, |_, _, cx| cx.notify());
        Self { page, clock, _subscription: subscription }
    }
}

impl Render for TakeButton {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.page.clone();
        ui::Btn::new("editor-stop")
            .danger()
            .small()
            .icon("circle-dot")
            .label(self.clock.read(cx).text.clone())
            .tooltip("Stop recording")
            .on_click(move |_, _, cx| {
                let _ = page.update(cx, |page, cx| page.stop_recording(cx));
            })
    }
}

/// A line of text that follows the playhead (the toolbar's clock, the take's
/// length), redrawn only when the text changes, not every frame.
pub struct Clock {
    text: SharedString,
    _subscriptions: Vec<Subscription>,
}

impl Clock {
    fn new(editor: Entity<EditorState>, cx: &mut Context<Self>, format: fn(&Playhead, &EditorState) -> String) -> Self {
        let playhead = editor.read(cx).playhead.clone();
        let update = move |this: &mut Self, editor: &Entity<EditorState>, cx: &mut Context<Self>| {
            let e = editor.read(cx);
            let text: SharedString = format(e.playhead.read(cx), e).into();
            if text != this.text {
                this.text = text;
                cx.notify();
            }
        };
        let text = format(playhead.read(cx), editor.read(cx)).into();
        let from_playhead = editor.clone();
        let subscriptions = vec![
            cx.observe(&playhead, move |this, _, cx| update(this, &from_playhead, cx)),
            cx.observe(&editor, move |this, editor, cx| update(this, &editor, cx)),
        ];
        Self { text, _subscriptions: subscriptions }
    }
}

impl Render for Clock {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(self.text.clone())
    }
}

