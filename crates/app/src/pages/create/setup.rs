//! First-run setup, inline on the Create page (SetupCard.tsx): pick an AI
//! provider and download the offline Whisper model here instead of being sent to
//! Settings. Hidden once both are ready and the tools check out. Manual clip mode
//! needs no provider, so there the card only asks for the Whisper model.
//!
//! The provider and model pickers are the Settings page's compact views; a build
//! without `page-settings` links to Settings in their place.
use super::CreatePage;
use crate::models::Settings;
use crate::theme::Tokens;
use crate::ui::{self, Btn, Hue, TypeScale, u};
use crate::workspace::{self, Page};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

#[derive(Default)]
pub struct SetupCard {
    #[cfg(feature = "page-settings")]
    provider: Option<Entity<crate::pages::settings::provider_setup::ProviderSetup>>,
    #[cfg(feature = "page-settings")]
    whisper: Option<Entity<crate::pages::settings::whisper_models::WhisperModels>>,
}

impl SetupCard {
    #[cfg(feature = "page-settings")]
    fn provider_view(&mut self, window: &mut Window, cx: &mut Context<CreatePage>) -> AnyElement {
        use crate::pages::settings::provider_setup::ProviderSetup;
        self.provider
            .get_or_insert_with(|| cx.new(|cx| ProviderSetup::new(true, window, cx)))
            .clone()
            .into_any_element()
    }

    #[cfg(feature = "page-settings")]
    fn whisper_view(&mut self, _: &mut Window, cx: &mut Context<CreatePage>) -> AnyElement {
        use crate::pages::settings::whisper_models::WhisperModels;
        self.whisper
            .get_or_insert_with(|| cx.new(|cx| WhisperModels::new(true, cx)))
            .clone()
            .into_any_element()
    }

    #[cfg(not(feature = "page-settings"))]
    fn provider_view(&mut self, _: &mut Window, _: &mut Context<CreatePage>) -> AnyElement {
        open_settings_button("setup-provider")
    }

    #[cfg(not(feature = "page-settings"))]
    fn whisper_view(&mut self, _: &mut Window, _: &mut Context<CreatePage>) -> AnyElement {
        open_settings_button("setup-whisper")
    }

    pub fn render(
        &mut self,
        manual: bool,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<CreatePage>,
    ) -> AnyElement {
        let settings = Settings::global(cx);
        let (setup, tool_status, tool_error) = {
            let s = settings.read(cx);
            (s.setup(), s.tool_status.clone(), s.tool_error.clone())
        };
        let needs_provider = !manual && !setup.provider_ready;
        if needs_provider || !setup.whisper_ready {
            let provider = (!manual).then(|| {
                setup_step(1, setup.provider_ready, "AI that picks the clips", t).when(
                    !setup.provider_ready,
                    |step| {
                        step.child(self.provider_view(window, cx)).child(
                            div().mt(u(8.)).text_caption().text_color(t.ink_secondary).child(
                                "Picking the moments yourself? Choose “I’ll choose the moments” in the Clips step and skip this.",
                            ),
                        )
                    },
                )
            });
            let whisper = setup_step(
                if manual { 1 } else { 2 },
                setup.whisper_ready,
                "Offline transcription (Whisper)",
                t,
            )
            .when(!setup.whisper_ready, |step| {
                step.child(self.whisper_view(window, cx)).child(
                    div().mt(u(8.)).text_caption().text_color(t.ink_secondary).child(
                        "Downloaded once. Your audio never leaves this computer. Other sizes are in Settings.",
                    ),
                )
            });
            return div()
                .mt(u(16.))
                .overflow_hidden()
                .rounded(u(12.))
                .border_1()
                .border_color(t.accent.opacity(0.25))
                .bg(t.accent.opacity(0.04))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(u(12.))
                        .px(u(16.))
                        .pb(u(4.))
                        .pt(u(16.))
                        .child(ui::icon_tile("sparkles", Hue::Accent, 32., t))
                        .child(
                            div()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_caption()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(t.accent_fg)
                                        .child("One-time setup"),
                                )
                                .child(div().mt(u(2.)).text_heading().text_color(t.ink).child(if manual {
                                    "One thing before your first clip"
                                } else {
                                    "Two things before your first clip"
                                }))
                                .child(div().mt(u(2.)).max_w(u(672.)).text_caption().text_color(t.ink_secondary).child(
                                    if manual {
                                        "You’re choosing the moments yourself, so no AI provider is needed. Download the speech model that transcribes offline for captions and pacing."
                                    } else {
                                        "Clipward has no account and no server. Pick the AI that chooses your clips, and download the speech model that transcribes offline."
                                    },
                                )),
                        ),
                )
                .children(provider)
                .child(whisper)
                .into_any_element();
        }

        if setup.tools_ok == Some(false) {
            let missing: Vec<&str> = tool_status
                .map(|s| {
                    [
                        (!s.smart_framing, "Smart framing model"),
                        (!s.ffmpeg, "FFmpeg"),
                        (!s.ffprobe, "FFprobe"),
                        (!s.ytdlp, "yt-dlp"),
                        (!s.engine, "Clipward clipping engine"),
                    ]
                    .into_iter()
                    .filter_map(|(missing, name)| missing.then_some(name))
                    .collect()
                })
                .unwrap_or_default();
            let (title, body) = if tool_error.is_some() {
                (
                    "Could not check required tools",
                    "Open Settings to retry the system check.".to_string(),
                )
            } else {
                (
                    "Some required tools are missing",
                    format!(
                        "Missing: {}. Open Settings for details.",
                        missing.join(", ")
                    ),
                )
            };
            return div()
                .mt(u(16.))
                .flex()
                .items_center()
                .gap(u(12.))
                .rounded(u(12.))
                .border_1()
                .border_color(t.danger.opacity(0.25))
                .bg(t.danger.opacity(0.06))
                .p(u(12.))
                .pr(u(14.))
                .child(ui::icon_tile("wrench", Hue::Danger, 32., t))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .child(div().text_label().text_color(t.ink).child(title))
                        .child(
                            div()
                                .mt(u(2.))
                                .text_caption()
                                .text_color(t.ink_secondary)
                                .child(body),
                        ),
                )
                .child(
                    Btn::new("see-missing")
                        .label("See what’s missing")
                        .on_click(|_, window, cx| workspace::go(Page::Settings, window, cx)),
                )
                .into_any_element();
        }
        div().into_any_element()
    }
}

/// One numbered step inside the setup card; a done step shows Ready and folds.
fn setup_step(n: u32, done: bool, title: &'static str, t: &Tokens) -> Div {
    div()
        .m(u(12.))
        .rounded(u(8.))
        .border_1()
        .border_color(t.stroke)
        .bg(t.bg.opacity(t.glass_strip))
        .p(u(12.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(u(8.))
                .when(!done, |row| row.mb(u(12.)))
                .child(
                    div()
                        .flex()
                        .size(u(16.))
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .text_micro()
                        .map(|c| {
                            if done {
                                c.bg(t.success)
                                    .child(ui::icon("check").size(u(10.)).text_color(t.bg))
                            } else {
                                c.bg(t.accent).text_color(t.accent_ink).child(n.to_string())
                            }
                        }),
                )
                .child(div().text_label().text_color(t.ink).child(title))
                .when(done, |row| {
                    row.child(div().text_caption().text_color(t.success).child("Ready"))
                }),
        )
}

#[cfg(not(feature = "page-settings"))]
fn open_settings_button(id: &'static str) -> AnyElement {
    div()
        .flex()
        .child(
            Btn::new(id)
                .label("Open Settings")
                .on_click(|_, window, cx| workspace::go(Page::Settings, window, cx)),
        )
        .into_any_element()
}
