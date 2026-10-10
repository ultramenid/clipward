//! The main window: sidebar on the left; on the right a 40px title bar over the
//! page, which scrolls on its own. Ports App.tsx, Layout.tsx and Sidebar.tsx.
//!
//! The window has a transparent title bar, so both top strips drag the window and
//! the sidebar's strip holds the macOS traffic lights.
use crate::models::{Jobs, Settings, SettingsChanged, UpdateState, Updater};
use crate::theme::{self, ActiveTokens, Tokens};
use crate::ui::{self, Tone, TypeScale, Truncate, u};
use clipward_core::jobs::JobSnapshot;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Create,
    Library,
    Jobs,
    Settings,
}

struct NavItem {
    page: Page,
    label: &'static str,
    icon: &'static str,
    shortcut: &'static str,
    studio: bool,
}

const NAV_ITEMS: [NavItem; 4] = [
    NavItem { page: Page::Create, label: "Create", icon: "wand-sparkles", shortcut: "1", studio: true },
    NavItem { page: Page::Library, label: "Library", icon: "film", shortcut: "2", studio: true },
    NavItem { page: Page::Jobs, label: "Jobs", icon: "layers", shortcut: "3", studio: true },
    NavItem { page: Page::Settings, label: "Settings", icon: "settings", shortcut: ",", studio: false },
];

/// Width of the traffic-light inset on macOS (overlay title bar).
const TRAFFIC_LIGHT_INSET: f32 = 78.;
const SIDEBAR_EXPANDED: f32 = 220.;
const SIDEBAR_RAIL: f32 = 52.;
/// Below this window width (CSS px) the sidebar is always the rail.
const WIDE_MIN: f32 = 1024.;

actions!(
    workspace,
    [GoCreate, GoLibrary, GoJobs, GoSettings, ToggleSidebar]
);

/// ⌘1 Create, ⌘2 Library, ⌘3 Jobs, ⌘, Settings, ⌘\ sidebar (Ctrl elsewhere).
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-1", GoCreate, None),
        KeyBinding::new("secondary-2", GoLibrary, None),
        KeyBinding::new("secondary-3", GoJobs, None),
        KeyBinding::new("secondary-,", GoSettings, None),
        KeyBinding::new("secondary-\\", ToggleSidebar, None),
    ]);
}

/// Switch the main window to `page` from anywhere inside it (a page's "View job").
pub fn go(page: Page, window: &mut Window, cx: &mut App) {
    let action: Box<dyn Action> = match page {
        Page::Create => Box::new(GoCreate),
        Page::Library => Box::new(GoLibrary),
        Page::Jobs => Box::new(GoJobs),
        Page::Settings => Box::new(GoSettings),
    };
    window.dispatch_action(action, cx);
}

/// Human labels for job stages (JobProgress.tsx STAGE_LABELS).
pub use clipward_core::jobs_page::stage_label;

pub struct Workspace {
    page: Page,
    content: AnyView,
    settings: Entity<Settings>,
    jobs: Entity<Jobs>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = Settings::global(cx);
        let jobs = Jobs::global(cx);
        let subscriptions = vec![
            cx.observe(&settings, |_, _, cx| cx.notify()),
            cx.observe(&jobs, |_, _, cx| cx.notify()),
            cx.observe(&Updater::global(cx), |_, _, cx| cx.notify()),
            cx.subscribe_in(&settings, window, |_, settings, _: &SettingsChanged, window, cx| {
                apply_appearance(&settings, window, cx);
            }),
            // The OS switching light/dark matters when the theme follows the system.
            cx.observe_window_appearance(window, |this, window, cx| {
                apply_appearance(&this.settings.clone(), window, cx);
            }),
        ];
        let mut this = Self {
            page: Page::Create,
            content: cx.new(|_| crate::pages::Placeholder(Page::Create)).into(),
            settings,
            jobs,
            _subscriptions: subscriptions,
        };
        this.navigate(Page::Create, window, cx);
        this
    }

    pub fn navigate(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        // Each visit starts fresh, as the React pages remounted.
        self.content = crate::pages::open(page, window, cx);
        cx.notify();
    }

    fn rail(&self, window: &Window, cx: &App) -> bool {
        let collapsed = self.settings.read(cx).appearance().collapsed_rail;
        collapsed || !self.wide(window)
    }

    fn wide(&self, window: &Window) -> bool {
        let scale = f32::from(window.rem_size()) / 16.0;
        f32::from(window.viewport_size().width) / scale >= WIDE_MIN
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            let mut appearance = settings.appearance();
            appearance.collapsed_rail = !appearance.collapsed_rail;
            settings.set_appearance(appearance, cx);
        });
    }

    fn render_sidebar(&self, rail: bool, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let active_jobs: Vec<JobSnapshot> =
            self.jobs.read(cx).active().into_iter().cloned().collect();
        let nav_button = |item: &NavItem, cx: &mut Context<Self>| {
            let active = self.page == item.page;
            let badge = (item.page == Page::Jobs && !active_jobs.is_empty())
                .then_some(active_jobs.len());
            let page = item.page;
            div()
                .id(item.label)
                .relative()
                .h(u(32.))
                .w_full()
                .flex()
                .items_center()
                .gap(u(8.))
                .rounded(u(6.))
                .map(|row| if rail { row.justify_center() } else { row.px(u(8.)) })
                .map(|row| {
                    if active {
                        row.bg(t.selection).text_color(t.ink)
                    } else {
                        row.text_color(t.ink_secondary)
                            .hover(|row| row.bg(t.tint(0.1)).text_color(t.ink))
                    }
                })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| this.navigate(page, window, cx)))
                .child(
                    ui::icon_sm(item.icon)
                        .text_color(if active { t.ink } else { t.ink_secondary }),
                )
                .when(rail && badge.is_some(), |row| {
                    row.child(
                        div()
                            .absolute()
                            .right(u(6.))
                            .top(u(6.))
                            .size(u(6.))
                            .rounded_full()
                            .bg(t.accent),
                    )
                })
                .when(!rail, |row| {
                    row.child(div().flex_1().min_w_0().text_label().one_line().child(item.label))
                        .child(match badge {
                            Some(count) => div()
                                .min_w(u(16.))
                                .px(u(4.))
                                .rounded_full()
                                .bg(t.accent)
                                .text_micro()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.accent_ink)
                                .flex()
                                .justify_center()
                                .child(if count > 99 { "99+".to_string() } else { count.to_string() })
                                .into_any_element(),
                            None => ui::kbd(format!("{}{}", ui::MOD_KEY, item.shortcut), t)
                                .into_any_element(),
                        })
                })
        };
        let group = |label: &'static str, studio: bool, cx: &mut Context<Self>| {
            div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .map(|group| {
                    if !rail {
                        group.child(
                            div()
                                .px(u(8.))
                                .pb(u(4.))
                                .text_caption()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.ink_tertiary)
                                .child(label),
                        )
                    } else if !studio {
                        // In the rail, a hairline replaces the label to separate groups.
                        group.child(div().mx(u(8.)).mb(u(4.)).h(px(1.)).bg(t.stroke))
                    } else {
                        group
                    }
                })
                .children(
                    NAV_ITEMS
                        .iter()
                        .filter(|item| item.studio == studio)
                        .map(|item| nav_button(item, cx))
                        .collect::<Vec<_>>(),
                )
        };
        let studio = group("Studio", true, cx);
        let app = group("App", false, cx);

        div()
            .id("sidebar")
            .flex()
            .flex_col()
            .flex_shrink_0()
            .h_full()
            .overflow_hidden()
            .w(u(if rail { SIDEBAR_RAIL } else { SIDEBAR_EXPANDED }))
            .bg(t.sidebar.opacity(t.glass_sidebar))
            .border_r_1()
            .border_color(t.stroke)
            // Title-bar strip: drag handle and home of the macOS traffic lights.
            .child(drag_strip().h(u(40.)).flex_shrink_0())
            .child(
                div()
                    .id("nav")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap(u(20.))
                    .overflow_y_scroll()
                    .px(u(8.))
                    .py(u(12.))
                    .child(studio)
                    .child(app),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .p(u(8.))
                    .when(self.page != Page::Jobs, |footer| {
                        footer.child(self.active_jobs_card(&active_jobs, rail, t, cx))
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(t.stroke)
                    .p(u(8.))
                    .children(self.update_status(rail, t, cx))
                    .child(self.setup_status(rail, t, cx)),
            )
    }

    /// Live jobs at a glance: one job shows its stage; several show a count and
    /// their average progress.
    fn active_jobs_card(
        &self,
        jobs: &[JobSnapshot],
        rail: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if jobs.is_empty() {
            return div().into_any_element();
        }
        let running: Vec<&JobSnapshot> = jobs.iter().filter(|j| j.status != "queued").collect();
        let queued = jobs.len() - running.len();
        let percent = if running.is_empty() {
            0.0
        } else {
            (running.iter().map(|j| j.percent).sum::<f64>() / running.len() as f64).round()
        };
        let single = (jobs.len() == 1).then(|| &jobs[0]);
        let title = match single {
            Some(job) => stage_label(&job.status).to_string(),
            None if queued > 0 => format!("{} running · {queued} queued", running.len()),
            None => format!("{} running", running.len()),
        };
        let subtitle = single
            .map(|job| clipward_core::format::source_label(&job.request.video_url))
            .unwrap_or_else(|| "View all jobs".into());
        ui::tile(t, false)
            .id("active-jobs")
            .w_full()
            .cursor_pointer()
            .hover(|tile| tile.bg(t.tint(0.06)))
            .map(|tile| if rail { tile.p(u(6.)) } else { tile.px(u(10.)).py(u(8.)) })
            .on_click(cx.listener(|this, _, window, cx| this.navigate(Page::Jobs, window, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .text_caption()
                    .when(rail, |row| row.justify_center())
                    .when(!rail, |row| {
                        row.child(ui::status_dot(Tone::Accent, true, t)).child(
                            div()
                                .one_line()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.ink)
                                .child(title),
                        )
                    })
                    .child(
                        div()
                            .when(!rail, |d| d.ml_auto())
                            .text_meta()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.accent_fg)
                            .child(format!("{percent}%")),
                    ),
            )
            .child(
                div()
                    .mt(u(if rail { 6. } else { 8. }))
                    .child(ui::progress_bar(percent, 2., t)),
            )
            .when(!rail, |tile| {
                tile.child(
                    div()
                        .mt(u(6.))
                        .text_meta()
                        .text_color(t.ink_secondary)
                        .one_line()
                        .child(subtitle),
                )
            })
            .into_any_element()
    }

    /// A new version, above the setup status so a setup problem can't hide it.
    /// One click installs and restarts, like Settings → Help & about.
    fn update_status(&self, rail: bool, t: &Tokens, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let updater = Updater::global(cx);
        let (label, detail, clickable) = match &updater.read(cx).state {
            UpdateState::Available(update) => (format!("Update to v{}", update.version), "Installs and restarts".into(), true),
            UpdateState::Failed { update: Some(update), .. } => {
                (format!("Update to v{}", update.version), "Failed, click to retry".into(), true)
            }
            UpdateState::Downloading { received, total, .. } => (
                "Downloading update".into(),
                format!("{}%", (*received * 100).checked_div(*total).unwrap_or(0).min(100)),
                false,
            ),
            UpdateState::Installing(_) => ("Restarting…".into(), String::new(), false),
            _ => return None,
        };
        Some(
            footer_row("update-status", rail, t)
                .mb(u(2.))
                .text_color(t.ink)
                .when(clickable, |row| row.on_click(move |_, _, cx| updater.update(cx, |u, cx| u.install(cx))))
                .child(div().size(u(16.)).flex().items_center().justify_center().child(ui::status_dot(Tone::Accent, !clickable, t)))
                .when(!rail, |row| {
                    row.child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(div().text_body().line_height(u(16.)).one_line().child(label))
                            .child(div().text_meta().text_color(t.ink_tertiary).one_line().child(detail)),
                    )
                }),
        )
    }

    fn setup_status(&self, rail: bool, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let setup = self.settings.read(cx).setup();
        let (tone, label) = if !setup.provider_ready || !setup.whisper_ready {
            (
                Tone::Warning,
                if !setup.provider_ready {
                    "Choose an AI provider"
                } else {
                    "Download Whisper model"
                },
            )
        } else if setup.tools_ok == Some(false) {
            (Tone::Danger, "System check failed")
        } else if setup.tools_ok.is_none() {
            (Tone::Idle, "Checking system…")
        } else {
            (Tone::Success, "Ready to clip")
        };
        footer_row("setup-status", rail, t)
            .text_color(if setup.ready { t.ink_secondary } else { t.ink.opacity(0.8) })
            .on_click(cx.listener(|this, _, window, cx| this.navigate(Page::Settings, window, cx)))
            .child(
                div()
                    .size(u(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(ui::status_dot(tone, false, t)),
            )
            .when(!rail, |row| {
                row.child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(div().text_body().line_height(u(16.)).one_line().child(label))
                        .child(
                            div()
                                .text_meta()
                                .font_family(crate::MONO_FONT)
                                .text_color(t.ink_tertiary)
                                .child(concat!("v", env!("CARGO_PKG_VERSION"))),
                        ),
                )
            })
    }

    fn render_title_bar(&self, rail: bool, wide: bool, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let item = NAV_ITEMS.iter().find(|n| n.page == self.page).unwrap_or(&NAV_ITEMS[0]);
        // In the rail the traffic lights spill past the sidebar's edge.
        let inset = if cfg!(target_os = "macos") && rail {
            TRAFFIC_LIGHT_INSET - SIDEBAR_RAIL
        } else {
            0.
        };
        let toggle_label = if rail { "Expand sidebar" } else { "Collapse sidebar" };
        drag_strip()
            .flex()
            .flex_shrink_0()
            .h(u(40.))
            .items_center()
            .gap(u(4.))
            .border_b_1()
            .border_color(t.stroke)
            .pr(u(12.))
            .pl(u(6. + inset))
            .when(wide, |bar| {
                bar.child(
                    Button::new("toggle-sidebar")
                        .ghost()
                        .icon(ui::icon("panel-left"))
                        .tooltip(format!("{toggle_label} ({}\\)", ui::MOD_KEY))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_sidebar(&ToggleSidebar, window, cx)
                        })),
                )
            })
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(u(8.))
                    .text_body()
                    .px(u(if wide { 6. } else { 8. }))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(t.ink_secondary)
                            .child(if item.studio { "Studio" } else { "App" }),
                    )
                    .child(div().text_color(t.tint(0.25)).child("›"))
                    .child(div().min_w_0().one_line().text_color(t.ink).child(item.label)),
            )
    }
}

/// A strip that moves the window when dragged and zooms it on double-click.
fn drag_strip() -> Div {
    div()
        .on_mouse_down(MouseButton::Left, |event, window, _| {
            if event.click_count == 2 {
                window.titlebar_double_click();
            } else {
                window.start_window_move();
            }
        })
}

/// Apply Settings → Appearance (theme, colours, scale, glass) to the window.
pub fn apply_appearance(settings: &Entity<Settings>, window: &mut Window, cx: &mut App) {
    let Some(value) = settings.read(cx).value.clone() else {
        return;
    };
    let appearance = &value.appearance;
    let dark = theme::resolve_dark(appearance.theme, window.appearance());
    let glass = value.window_glass && value.window_glass_supported;
    let blurred = glass && dark;
    window.set_background_appearance(if blurred {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Opaque
    });
    #[cfg(target_os = "macos")]
    if blurred {
        crate::glass::set_blur(window, crate::glass::blur_radius(appearance.blur_radius));
    }
    // Tokens::install hands the rem (16 × interface scale) to the UI kit's theme,
    // whose root applies it to the window every frame.
    Tokens::new(appearance, dark, glass).install(cx);
    // Cached views (the editor's panels, the Jobs lists) draw again with the new look.
    window.refresh();
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        let wide = self.wide(window);
        let rail = self.rail(window, cx);
        div()
            .id("workspace")
            .key_context("Workspace")
            .on_action(cx.listener(|this, _: &GoCreate, w, cx| this.navigate(Page::Create, w, cx)))
            .on_action(cx.listener(|this, _: &GoLibrary, w, cx| this.navigate(Page::Library, w, cx)))
            .on_action(cx.listener(|this, _: &GoJobs, w, cx| this.navigate(Page::Jobs, w, cx)))
            .on_action(cx.listener(|this, _: &GoSettings, w, cx| this.navigate(Page::Settings, w, cx)))
            .on_action(cx.listener(Self::toggle_sidebar))
            .flex()
            .size_full()
            .overflow_hidden()
            .font_family(crate::UI_FONT)
            .text_color(t.ink)
            .text_body()
            .child(self.render_sidebar(rail, &t, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .bg(t.bg.opacity(t.glass_window))
                    .child(self.render_title_bar(rail, wide, &t, cx))
                    .child(div().flex_1().min_h_0().child(self.content.clone())),
            )
    }
}

/// A clickable row in the sidebar footer; icon-only in the rail.
fn footer_row(id: &'static str, rail: bool, t: &Tokens) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .w_full()
        .min_w_0()
        .items_center()
        .gap(u(10.))
        .rounded(u(6.))
        .cursor_pointer()
        .hover(|row| row.bg(t.tint(0.1)).text_color(t.ink))
        .map(|row| if rail { row.h(u(32.)).justify_center() } else { row.px(u(8.)).py(u(6.)) })
}
