//! The text, overlay, picture-in-picture and sound inspectors and the overlays list
//! (TextPanels.tsx TextInspector, OverlayPanels.tsx, SoundPanels.tsx).
use super::{Inspector, Pick, header, hint, label, section};
use crate::editor::state::{EditorState, OVERLAY_SCALE_MAX, OVERLAY_SCALE_MIN, Selection};
use crate::theme::ActiveTokens;
use crate::ui::{self, Btn, TypeScale, u};
use crate::{shell, state};
use clipward_core::clip_engine::render::project::{
    AudioItem, OverlayBox, PIP_WIDTH_RANGE, PipItem, Project, TEXT_SIZE_RANGE, TextLayer,
};
use clipward_core::clip_engine::render::service::{MAX_LOGO_SIZE, MIN_LOGO_SIZE};
use clipward_core::inspector::{fade_label, fade_max, volume_label};
use clipward_core::overlays::{OverlayKey, OverlayView};
use clipward_core::timeline::js_round;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::{Sizable, Size};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// Most logos a clip takes in the editor.
const MAX_EDITOR_LOGOS: usize = 8;

fn pct(v: f64) -> f64 {
    js_round(v * 100.)
}

fn edit_text(e: &mut EditorState, id: &str, patch: impl Fn(&mut TextLayer), record: bool, cx: &mut Context<EditorState>) {
    e.edit_texts(
        |texts| {
            texts
                .iter()
                .map(|t| {
                    let mut t = t.clone();
                    if t.id == id {
                        patch(&mut t);
                    }
                    t
                })
                .collect()
        },
        record,
        cx,
    );
}

fn edit_sound(e: &mut EditorState, id: &str, patch: impl Fn(&mut AudioItem), cx: &mut Context<EditorState>) {
    e.edit_sounds(
        |audio| {
            audio
                .iter()
                .map(|a| {
                    let mut a = a.clone();
                    if a.id == id {
                        patch(&mut a);
                    }
                    a
                })
                .collect()
        },
        true,
        cx,
    );
}

fn edit_pip(e: &mut EditorState, id: &str, patch: impl Fn(&mut PipItem), cx: &mut Context<EditorState>) {
    e.edit_pip(
        |pip| {
            pip.iter()
                .map(|p| {
                    let mut p = p.clone();
                    if p.id == id {
                        patch(&mut p);
                    }
                    p
                })
                .collect()
        },
        true,
        cx,
    );
}

impl Inspector {
    /// Fade-in and fade-out sliders for something `len` ms long.
    pub(super) fn fade_sliders(
        &mut self,
        keys: (&'static str, &'static str),
        (fade_in, fade_out): (i64, i64),
        len: i64,
        apply: impl Fn(&mut EditorState, Option<i64>, Option<i64>, &mut Context<EditorState>) + Clone + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> [AnyElement; 2] {
        let max = fade_max(len) as f64;
        let set_in = apply.clone();
        [
            self.slider(keys.0, "Fade in", (fade_in as f64).min(max), (0., max, 100.), fade_label(fade_in), move |e, v, cx| set_in(e, Some(v as i64), None, cx), window, cx),
            self.slider(keys.1, "Fade out", (fade_out as f64).min(max), (0., max, 100.), fade_label(fade_out), move |e, v, cx| apply(e, None, Some(v as i64), cx), window, cx),
        ]
    }

    pub(super) fn text_inspector(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let layer = self.editor.read(cx).project()?.texts.iter().find(|t| t.id == id)?.clone();
        let t = *cx.tokens();
        let (min, max) = TEXT_SIZE_RANGE;
        let size_id = layer.id.clone();
        let size = self.slider(
            "text-size",
            "Size",
            layer.size as f64,
            (min as f64, max as f64, 4.),
            layer.size.to_string(),
            move |e, v, cx| edit_text(e, &size_id, |t| t.size = v as i64, true, cx),
            window,
            cx,
        );
        let style = self.select(Pick::Style, &layer.preset, "Text style", window, cx);
        let entrance = self.select(Pick::In, &layer.animation_in, "Entrance", window, cx);
        let exit = self.select(Pick::Out, &layer.animation_out, "Exit", window, cx);
        Some(
            section(&t)
                .child(header("Text", Some(("delete-text", "Delete text (Delete)")), &self.editor, &t))
                .child(Textarea::new(&self.text).text_caption())
                .child(style)
                .child(size)
                .child(
                    div()
                        .flex()
                        .gap(u(8.))
                        .child(div().flex_1().min_w_0().flex().flex_col().gap(u(2.)).child(label("In", &t)).child(entrance))
                        .child(div().flex_1().min_w_0().flex().flex_col().gap(u(2.)).child(label("Out", &t)).child(exit)),
                )
                .child(hint("Drag the text on the video to move it; drag its edges on the timeline to time it.", &t))
                .into_any_element(),
        )
    }

    pub(super) fn overlay_inspector(&mut self, key: OverlayKey, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let editor = self.editor.read(cx);
        let project = editor.project()?;
        // The title keeps its inspector while it is retyped (an empty title draws nothing).
        let view = match editor.overlays.iter().find(|o| o.key == key) {
            Some(view) => view.clone(),
            None if key == OverlayKey::Title && project.title.is_some() => OverlayView {
                key,
                label: "Title".into(),
                box_: project.title_box.clone().unwrap_or(OverlayBox {
                    x: 0.5,
                    y: 0.1,
                    scale: 1.,
                    start_ms: 0,
                    end_ms: editor.total() as i64,
                }),
                placed: project.title_box.is_some(),
                path: Default::default(),
                width: 0.,
                height: 0.,
            },
            None => return None,
        };
        let logo_size = key.logo_index().and_then(|i| project.logos.get(i)).map(|l| l.size);
        let t = *cx.tokens();
        let size = match (key.logo_index(), logo_size) {
            (Some(index), Some(size)) => self.slider(
                "logo-size",
                "Size",
                pct(size),
                (MIN_LOGO_SIZE * 100., MAX_LOGO_SIZE * 100., 1.),
                format!("{}%", pct(size)),
                move |e, v, cx| {
                    e.change(
                        |p| {
                            let mut logos = p.logos.clone();
                            logos.get_mut(index)?.size = v / 100.;
                            (logos != p.logos).then(|| Project { logos, ..p.clone() })
                        },
                        true,
                        cx,
                    )
                },
                window,
                cx,
            ),
            _ => self.slider(
                "overlay-scale",
                "Size",
                pct(view.box_.scale),
                (OVERLAY_SCALE_MIN * 100., OVERLAY_SCALE_MAX * 100., 5.),
                format!("{}%", pct(view.box_.scale)),
                move |e, v, cx| e.place_overlay(key, |b| b.scale = v / 100., true, cx),
                window,
                cx,
            ),
        };
        let editor = self.editor.clone();
        Some(
            section(&t)
                .child(header(
                    if key == OverlayKey::Title { "Title".to_string() } else { view.label.clone() },
                    Some(("delete-overlay", "Remove (Delete)")),
                    &self.editor,
                    &t,
                ))
                .when(key == OverlayKey::Title, |d| {
                    d.child(Input::new(&self.title).with_size(Size::Small).text_caption())
                })
                .child(size)
                .child(if view.placed {
                    div()
                        .flex()
                        .child(
                            Btn::new("overlay-auto")
                                .small()
                                .icon("rotate-ccw")
                                .label("Put back automatically")
                                .on_click(move |_, _, cx| editor.update(cx, |e, cx| e.set_overlay_box(key, None, true, cx))),
                        )
                        .into_any_element()
                } else {
                    hint("Placed automatically. Drag it on the video to move it, or its edges on the timeline to time it.", &t)
                        .into_any_element()
                })
                .into_any_element(),
        )
    }

    pub(super) fn pip_inspector(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let editor = self.editor.read(cx);
        let item = editor.project()?.pip.iter().find(|p| p.id == id)?.clone();
        let file = editor.media_file(&item.media).cloned();
        let t = *cx.tokens();
        let (min, max) = PIP_WIDTH_RANGE;
        let width_id = item.id.clone();
        let size = self.slider(
            "pip-size",
            "Size",
            pct(item.width),
            (min * 100., max * 100., 1.),
            format!("{}%", pct(item.width)),
            move |e, v, cx| edit_pip(e, &width_id, |p| p.width = v / 100., cx),
            window,
            cx,
        );
        let volume = (!file.as_ref().is_some_and(|f| f.image)).then(|| {
            let volume_id = item.id.clone();
            self.slider(
                "pip-volume",
                "Volume",
                pct(item.volume),
                (0., 200., 5.),
                volume_label(item.volume),
                move |e, v, cx| edit_pip(e, &volume_id, |p| p.volume = v / 100., cx),
                window,
                cx,
            )
        });
        Some(
            section(&t)
                .child(header(
                    file.map_or_else(|| "Picture-in-picture".to_string(), |f| f.name),
                    Some(("delete-pip", "Remove (Delete)")),
                    &self.editor,
                    &t,
                ))
                .child(size)
                .children(volume)
                .child(hint("Drag it on the video to move it, or its corner to resize it.", &t))
                .into_any_element(),
        )
    }

    pub(super) fn sound_inspector(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let item = self.editor.read(cx).project()?.audio.iter().find(|a| a.id == id)?.clone();
        let t = *cx.tokens();
        let volume_id = item.id.clone();
        let volume = self.slider(
            "sound-volume",
            "Volume",
            pct(item.volume),
            (0., 200., 5.),
            volume_label(item.volume),
            move |e, v, cx| edit_sound(e, &volume_id, |a| a.volume = v / 100., cx),
            window,
            cx,
        );
        let fade_id = item.id.clone();
        let fades = self.fade_sliders(
            ("sound-fade-in", "sound-fade-out"),
            (item.fade_in_ms, item.fade_out_ms),
            item.src_out_ms - item.src_in_ms,
            move |e, fade_in, fade_out, cx| {
                edit_sound(
                    e,
                    &fade_id,
                    |a| {
                        a.fade_in_ms = fade_in.unwrap_or(a.fade_in_ms);
                        a.fade_out_ms = fade_out.unwrap_or(a.fade_out_ms);
                    },
                    cx,
                )
            },
            window,
            cx,
        );
        let duck = (item.file.is_some() && !item.voice).then(|| {
            let editor = self.editor.clone();
            let duck_id = item.id.clone();
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(u(12.))
                .child(
                    div()
                        .child(div().text_caption().text_color(t.ink).child("Lower under speech"))
                        .child(div().text_meta().text_color(t.ink_tertiary).child("Drops while someone talks")),
                )
                .child(ui::switch(
                    "sound-duck",
                    item.duck,
                    false,
                    move |duck, _, cx| editor.update(cx, |e, cx| edit_sound(e, &duck_id, |a| a.duck = duck, cx)),
                    &t,
                ))
        });
        Some(
            section(&t)
                .child(header(item.name.clone(), Some(("delete-sound", "Delete sound (Delete)")), &self.editor, &t))
                .child(volume)
                .children(fades)
                .children(duck)
                .into_any_element(),
        )
    }

    /// The clip's title, banner and logos: pick one to move, resize or time it.
    pub(super) fn overlays_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = *cx.tokens();
        let editor = self.editor.read(cx);
        let editable = editor.editable();
        let selected = match &editor.selection {
            Some(Selection::Overlay(key)) => Some(*key),
            _ => None,
        };
        let (no_title, logos) = editor.project().map_or((false, 0), |p| (p.title.is_none(), p.logos.len()));
        let rows: Vec<_> = editor
            .overlays
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let key = o.key;
                let chosen = selected == Some(key);
                let editor = self.editor.clone();
                div()
                    .id(("overlay-row", i))
                    .flex()
                    .w_full()
                    .items_center()
                    .gap(u(8.))
                    .rounded(u(6.))
                    .px(u(8.))
                    .py(u(4.))
                    .text_caption()
                    .cursor_pointer()
                    .map(|d| if chosen { d.bg(t.selection).text_color(t.ink) } else { d.text_color(t.ink_secondary).hover(|s| s.bg(t.tint(0.1))) })
                    .on_click(move |_, _, cx| editor.update(cx, |e, cx| e.select(Some(Selection::Overlay(key)), cx)))
                    .child(ui::icon("layers").size(u(14.)).flex_shrink_0())
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(o.label.clone()))
                    .when(!o.placed, |d| {
                        d.child(div().ml_auto().flex_shrink_0().text_meta().text_color(t.ink_tertiary).child("Auto"))
                    })
            })
            .collect();
        let has_rows = !rows.is_empty();
        let add_title = self.editor.clone();
        section(&t)
            .gap(u(8.))
            .child(div().text_label().text_color(t.ink).child("Overlays"))
            .when(has_rows, |d| d.child(div().flex().flex_col().gap(u(2.)).children(rows)))
            .child(
                div()
                    .flex()
                    .gap(u(4.))
                    .when(no_title, |d| {
                        d.child(
                            Btn::new("add-title")
                                .ghost()
                                .small()
                                .icon("type")
                                .label("Add title")
                                .disabled(!editable)
                                .on_click(move |_, _, cx| add_title.update(cx, |e, cx| e.add_title(cx))),
                        )
                    })
                    .child(
                        Btn::new("add-logo")
                            .ghost()
                            .small()
                            .icon("image-plus")
                            .label("Add logo")
                            .loading(self.adding_logo)
                            .disabled(!editable || logos >= MAX_EDITOR_LOGOS)
                            .on_click(cx.listener(|this, _, _, cx| this.add_logo(cx))),
                    ),
            )
            .into_any_element()
    }

    /// Pick a logo image, copy it into the run and put it top right.
    fn add_logo(&mut self, cx: &mut Context<Self>) {
        if self.adding_logo {
            return;
        }
        self.adding_logo = true;
        cx.notify();
        let dir = self.editor.read(cx).output_dir.clone();
        cx.spawn(async move |this, cx| {
            let filters = [shell::Filter("Images", clipward_core::shared::LOGO_EXTENSIONS)];
            let path = shell::pick_file("Choose a Logo", &filters).await;
            let _ = this.update(cx, |this, cx| {
                this.adding_logo = false;
                if let Some(path) = path {
                    let imported = state::core(cx).project_import_logo(&dir, &path);
                    this.editor.update(cx, |e, cx| match imported {
                        Ok(name) => e.add_logo(name, cx),
                        Err(message) => {
                            e.save_error = Some(if message.is_empty() { "Could not add that logo.".into() } else { message });
                            cx.notify();
                        }
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }
}
