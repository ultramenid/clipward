//! Development only (`--features snapshot`): with `CLIPWARD_SNAPSHOT=<dir>` set,
//! visit each page, save the window as `<dir>/<page>.png`, then quit.
//!
//! `CLIPWARD_SNAPSHOT_EDITOR=<run folder>` instead opens the clip editor on that run
//! (see `editor_probe`); nothing is edited or saved.
use crate::workspace::{Page, Workspace};
use gpui_kit::*;
use std::path::PathBuf;
use std::time::Duration;

pub fn tour(workspace: Entity<Workspace>, window: &mut Window, cx: &mut App) {
    let Some(dir) = std::env::var_os("CLIPWARD_SNAPSHOT").map(PathBuf::from) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        for (page, name) in [
            (Page::Create, "create"),
            (Page::Library, "library"),
            (Page::Jobs, "jobs"),
            (Page::Settings, "settings"),
        ] {
            let _ = handle.update(cx, |_, window, cx| {
                workspace.update(cx, |w, cx| w.navigate(page, window, cx));
            });
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            let path = dir.join(format!("{name}.png"));
            // The window is hidden, so nothing draws it on its own: draw it a few times so
            // layout that settles on the next frame (measured grids, images) is in.
            for _ in 0..4 {
                let _ = handle.update(cx, |_, window, cx| window.draw(cx).clear(cx));
                cx.background_executor().timer(Duration::from_millis(100)).await;
            }
            let _ = handle.update(cx, |_, window, _| match window.render_to_image() {
                Ok(image) => {
                    let _ = image.save(&path);
                }
                Err(error) => eprintln!("snapshot {name}: {error}"),
            });
        }
        cx.update(|cx| cx.quit());
    })
    .detach();
}

/// `CLIPWARD_VIDEO_PROBE=<file>`: play the file in a hidden window and save
/// `<dir>/video.png` after two seconds, with the frames drawn so far, then quit.
pub fn video_probe(cx: &mut App) -> bool {
    let (Some(dir), Some(file)) = (
        std::env::var_os("CLIPWARD_SNAPSHOT").map(PathBuf::from),
        std::env::var_os("CLIPWARD_VIDEO_PROBE").map(PathBuf::from),
    ) else {
        return false;
    };
    let _ = std::fs::create_dir_all(&dir);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(540.), px(960.)), cx)),
        show: false,
        focus: false,
        ..Default::default()
    };
    let window = cx
        .open_window(options, |_, cx| {
            cx.new(|_| {
                let mut player = crate::video::MediaPlayer::new();
                player.open(Some(&file), true);
                VideoProbe { player, frames: 0 }
            })
        })
        .expect("open the probe window");
    cx.spawn(async move |cx| {
        // Hidden windows get no display link: tick at 60 Hz instead.
        let seconds: u32 = std::env::var("CLIPWARD_VIDEO_PROBE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2);
        for _ in 0..seconds * 60 {
            cx.background_executor().timer(Duration::from_millis(16)).await;
            let _ = window.update(cx, |_, _, cx| cx.notify());
            let any: AnyWindowHandle = window.into();
            let _ = any.update(cx, |_, window, cx| window.draw(cx).clear(cx));
        }
        // Draw through the plain window handle: updating the view itself would hold it
        // while the frame renders it.
        let any: AnyWindowHandle = window.into();
        let _ = any.update(cx, |_, window, cx| window.draw(cx).clear(cx));
        let _ = window.update(cx, |probe, window, _| {
            eprintln!(
                "video probe: ready={} frames={} at={:.0}ms duration={:?}",
                probe.player.ready(),
                probe.frames,
                probe.player.current_ms(),
                probe.player.duration_ms()
            );
            match window.render_to_image() {
                Ok(image) => {
                    let _ = image.save(dir.join("video.png"));
                }
                Err(error) => eprintln!("video probe: {error}"),
            }
        });
        cx.update(|cx| cx.quit());
    })
    .detach();
    true
}

struct VideoProbe {
    player: crate::video::MediaPlayer,
    frames: u32,
}

impl Render for VideoProbe {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if self.player.ready() && !self.player.is_playing() {
            self.player.set_playing(true, 1.0);
        }
        // Loop, so a long probe keeps decoding.
        if self.player.ended() && !self.player.is_seeking() {
            self.player.seek(0.0, true);
            self.player.set_playing(false, 1.0);
        }
        if self.player.poll_frame(window) {
            self.frames += 1;
        }
        div()
            .size_full()
            .bg(black())
            .children(self.player.surface(ObjectFit::Contain).map(|s| s.size_full()))
    }
}

/// `CLIPWARD_SNAPSHOT_EDITOR=<run folder>`: open the editor on a clip in a hidden
/// 1280×800 window and save `<dir>/editor.png`, then quit. Options:
/// `CLIPWARD_SNAPSHOT_CLIP=<index>` (0), `CLIPWARD_SNAPSHOT_SELECT=piece:0 | text:<id>
/// | sound:<id> | pip:<id> | overlay:title | overlay:banner | overlay:logo:<n>`,
/// `CLIPWARD_SNAPSHOT_AT=<ms>` (playhead), `CLIPWARD_SNAPSHOT_WIDTH=<px>` (1280), `CLIPWARD_SNAPSHOT_SECS=<n>` (3: time to
/// load and draw), `CLIPWARD_SNAPSHOT_WARMUP=1` (keep the loading screen),
/// `CLIPWARD_SNAPSHOT_PLAY=1` (play once loaded; prints how far the playhead got and
/// how long the frames took to draw; `full` redraws every view each frame).
pub fn editor_probe(cx: &mut App) -> bool {
    use crate::editor::state::Selection;
    use clipward_core::overlays::OverlayKey;
    let (Some(dir), Some(run)) = (
        std::env::var_os("CLIPWARD_SNAPSHOT").map(PathBuf::from),
        std::env::var("CLIPWARD_SNAPSHOT_EDITOR").ok(),
    ) else {
        return false;
    };
    let _ = std::fs::create_dir_all(&dir);
    let var = |name: &str| std::env::var(name).ok();
    let clip: u32 = var("CLIPWARD_SNAPSHOT_CLIP").and_then(|s| s.parse().ok()).unwrap_or(0);
    let seconds: u32 = var("CLIPWARD_SNAPSHOT_SECS").and_then(|s| s.parse().ok()).unwrap_or(3);
    let at: Option<f64> = var("CLIPWARD_SNAPSHOT_AT").and_then(|s| s.parse().ok());
    let warmup = var("CLIPWARD_SNAPSHOT_WARMUP").as_deref() == Some("1");
    let play = var("CLIPWARD_SNAPSHOT_PLAY").is_some_and(|v| v == "1" || v == "full");
    // `full`: time frames that redraw every view (the worst case), not just what changed.
    let full = var("CLIPWARD_SNAPSHOT_PLAY").as_deref() == Some("full");
    let selection = var("CLIPWARD_SNAPSHOT_SELECT").and_then(|s| {
        let (kind, rest) = s.split_once(':')?;
        Some(match (kind, rest) {
            ("piece", i) => Selection::Piece(i.parse().ok()?),
            ("text", id) => Selection::Text(id.into()),
            ("sound", id) => Selection::Sound(id.into()),
            ("pip", id) => Selection::Pip(id.into()),
            ("overlay", "title") => Selection::Overlay(OverlayKey::Title),
            ("overlay", "banner") => Selection::Overlay(OverlayKey::Banner),
            ("overlay", logo) => Selection::Overlay(OverlayKey::Logo(logo.strip_prefix("logo:")?.parse().ok()?)),
            _ => return None,
        })
    });
    let width: f32 = var("CLIPWARD_SNAPSHOT_WIDTH").and_then(|s| s.parse().ok()).unwrap_or(1280.);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(width), px(800.)), cx)),
        show: false,
        focus: false,
        ..Default::default()
    };
    let (window, probe) = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| EditorProbe {
            page: cx.new(|cx| crate::editor::EditorPage::new(run, clip, "Snapshot".into(), window, cx)),
        })
    })
    .expect("open the editor probe window");
    cx.spawn(async move |cx| {
        let mut prepared = false;
        let mut started: Option<(std::time::Instant, f64)> = None;
        let mut draws: Vec<Duration> = Vec::new();
        // Hidden windows get no display link: tick at 60 Hz instead.
        for _ in 0..seconds * 60 {
            cx.background_executor().timer(Duration::from_millis(16)).await;
            let _ = cx.update(|cx| {
                let editor = probe.read(cx).page.read(cx).editor().clone();
                if !prepared && editor.read(cx).project().is_some() {
                    prepared = true;
                    editor.update(cx, |e, cx| {
                        if !warmup {
                            e.warmup = None;
                        }
                        if let Some(at) = at {
                            e.seek(at, cx);
                        }
                        e.select(selection.clone(), cx);
                        cx.notify();
                    });
                }
                // Play once the loading screen is gone (or skipped) and the first frame is up.
                if play && prepared && started.is_none() && editor.read(cx).warmup.is_none() {
                    let at = editor.update(cx, |e, cx| {
                        e.set_playing(true, cx);
                        e.time(cx)
                    });
                    started = Some((std::time::Instant::now(), at));
                }
            });
            // No display link either. While playing, wake the player as a display link
            // would (only views that changed draw); otherwise redraw every view, cached
            // ones included, so loading shows.
            let drawn = std::time::Instant::now();
            let live = started.is_some() && !full;
            let _ = cx.update(|cx| {
                if live {
                    let player = probe.read(cx).page.read(cx).player().clone();
                    player.update(cx, |_, cx| cx.notify());
                }
            });
            let _ = window.update(cx, |_, window, cx| {
                if !live {
                    window.refresh();
                }
                window.draw(cx).clear(cx)
            });
            if started.is_some() {
                draws.push(drawn.elapsed());
            }
        }
        if let Some((at, from)) = started {
            let _ = cx.update(|cx| {
                let editor = probe.read(cx).page.read(cx).editor().clone();
                let e = editor.read(cx);
                let p = e.playhead.read(cx);
                draws.sort();
                let mean = draws.iter().sum::<Duration>().as_secs_f64() * 1000. / draws.len().max(1) as f64;
                eprintln!(
                    "editor probe: played {:.0} ms of edit in {:.0} ms wall (playing={}); draw mean {:.2} ms, p95 {:.2} ms, max {:.2} ms over {} frames",
                    p.time - from,
                    at.elapsed().as_secs_f64() * 1000.,
                    p.playing,
                    mean,
                    draws.get(draws.len() * 95 / 100).map_or(0., |d| d.as_secs_f64() * 1000.),
                    draws.last().map_or(0., |d| d.as_secs_f64() * 1000.),
                    draws.len(),
                );
            });
        }
        let _ = window.update(cx, |_, window, _| match window.render_to_image() {
            Ok(image) => {
                let _ = image.save(dir.join("editor.png"));
            }
            Err(error) => eprintln!("editor probe: {error}"),
        });
        cx.update(|cx| cx.quit());
    })
    .detach();
    true
}

struct EditorProbe {
    page: Entity<crate::editor::EditorPage>,
}

impl Render for EditorProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::theme::ActiveTokens;
        div().size_full().bg(cx.tokens().bg).text_color(cx.tokens().ink).child(self.page.clone())
    }
}
