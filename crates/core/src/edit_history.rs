//! The clip editor's open project with its undo and redo history (ported from the React
//! renderer's `use-editor-store.ts`; see docs/GPUI_PORT.md). The playhead is not here:
//! the shell keeps it, and resets it to 0 when it opens a project.
use clip_engine::render::project::Project;
use std::collections::VecDeque;

/// Undo steps kept per clip.
const MAX_HISTORY: usize = 100;

/// Where a batch of changes (a slider drag) is: none running, running with no undo step
/// yet, or running with its one step recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Batch {
    #[default]
    Off,
    Open,
    Recorded,
}

/// The open project and its history.
#[derive(Debug, Default)]
pub struct EditHistory {
    project: Option<Project>,
    /// Oldest first; the last is what undo goes back to.
    past: VecDeque<Project>,
    /// Next first: what redo goes forward to.
    future: VecDeque<Project>,
    revision: u64,
    batch: Batch,
}

impl EditHistory {
    /// The project being edited, if one is open.
    pub fn project(&self) -> Option<&Project> {
        self.project.as_ref()
    }

    /// Undo steps, oldest first.
    pub fn past(&self) -> &VecDeque<Project> {
        &self.past
    }

    /// Redo steps, next first.
    pub fn future(&self) -> &VecDeque<Project> {
        &self.future
    }

    /// Bumps on every change, so autosave can tell a new edit from a re-render.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn batch(&self) -> Batch {
        self.batch
    }

    /// Starts editing `project` with no history (the batch state is left as it was).
    pub fn open(&mut self, project: Project) {
        self.project = Some(project);
        self.past.clear();
        self.future.clear();
        self.revision = 0;
    }

    /// Closes the project and drops its history (the revision is left as it was).
    pub fn close(&mut self) {
        self.project = None;
        self.past.clear();
        self.future.clear();
    }

    /// Applies an edit: `update` returns the new project, or None when nothing changed
    /// (the TS compared object identity). `record: false` skips the undo step: a drag
    /// records one step when it starts (`checkpoint`) and then updates freely until it
    /// ends. Inside a batch only the first recorded change records.
    pub fn change(&mut self, update: impl FnOnce(&Project) -> Option<Project>, record: bool) {
        let Some(project) = &self.project else { return };
        let Some(next) = update(project) else { return };
        let previous = self.project.replace(next).expect("a project is open");
        let recording = record && self.batch != Batch::Recorded;
        if recording {
            self.push_past(previous);
            if self.batch == Batch::Open {
                self.batch = Batch::Recorded;
            }
        }
        self.future.clear();
        self.revision += 1;
    }

    /// Records the project as it is now as an undo step (the start of a drag).
    pub fn checkpoint(&mut self) {
        if let Some(project) = &self.project {
            let project = project.clone();
            self.push_past(project);
            self.future.clear();
        }
    }

    /// Groups the changes until `end_batch` into one undo step (a slider drag): only the
    /// first recorded change inside the batch records.
    pub fn begin_batch(&mut self) {
        self.batch = Batch::Open;
    }

    pub fn end_batch(&mut self) {
        self.batch = Batch::Off;
    }

    /// Goes back one step; does nothing with no project or nothing to undo.
    pub fn undo(&mut self) {
        let Some(project) = self.project.take() else {
            return;
        };
        let Some(previous) = self.past.pop_back() else {
            self.project = Some(project);
            return;
        };
        self.future.push_front(project);
        self.project = Some(previous);
        self.revision += 1;
    }

    /// Goes forward one step; does nothing with no project or nothing to redo. Like the
    /// TS, redo does not trim the history to MAX_HISTORY.
    pub fn redo(&mut self) {
        let Some(project) = self.project.take() else {
            return;
        };
        let Some(next) = self.future.pop_front() else {
            self.project = Some(project);
            return;
        };
        self.past.push_back(project);
        self.project = Some(next);
        self.revision += 1;
    }

    /// Adds an undo step, keeping the newest MAX_HISTORY.
    fn push_past(&mut self, project: Project) {
        self.past.push_back(project);
        while self.past.len() > MAX_HISTORY {
            self.past.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A project told apart by its speed (the TS test used a bare `{ volume }`).
    fn project(speed: f64) -> Project {
        serde_json::from_value(json!({
            "version": 1, "source_width": 1920, "source_height": 1080, "width": 1080, "height": 1920,
            "fps": "30", "aspect_ratio": "9:16", "video": [], "shots": [], "face_samples": [], "transcript": [],
            "captions": true, "caption_preset": "pop", "emphasis_words": [], "title": null,
            "banner_platform": null, "banner_channel_url": null, "logos": [], "speed": speed
        }))
        .unwrap()
    }

    fn set_speed(speed: f64) -> impl FnOnce(&Project) -> Option<Project> {
        move |p| Some(Project { speed, ..p.clone() })
    }

    fn speed(h: &EditHistory) -> f64 {
        h.project().unwrap().speed
    }

    #[test]
    fn a_slider_drag_is_one_undo_step() {
        let mut h = EditHistory::default();
        h.open(project(1.0));
        h.begin_batch();
        for s in [0.9, 0.8, 0.7] {
            h.change(set_speed(s), true);
        }
        h.end_batch();
        h.change(set_speed(0.5), true);
        assert_eq!(h.past().len(), 2);
        h.undo();
        assert_eq!(speed(&h), 0.7);
        h.undo();
        assert_eq!(speed(&h), 1.0);
    }

    #[test]
    fn redo_replays_and_a_new_edit_drops_it() {
        let mut h = EditHistory::default();
        h.undo();
        h.change(set_speed(2.0), true);
        assert!(h.project().is_none(), "nothing open: nothing happens");
        h.open(project(1.0));
        h.change(|_| None, true);
        assert_eq!(
            (h.past().len(), h.revision()),
            (0, 0),
            "an unchanged project is no step"
        );
        h.change(set_speed(2.0), true);
        h.change(set_speed(3.0), false);
        assert_eq!((h.past().len(), h.revision()), (1, 2));
        h.undo();
        assert_eq!((speed(&h), h.future().len()), (1.0, 1));
        h.redo();
        assert_eq!(
            (speed(&h), h.past().len(), h.future().len(), h.revision()),
            (3.0, 1, 0, 4)
        );
        h.redo();
        assert_eq!(h.revision(), 4, "nothing to redo");
        h.undo();
        h.checkpoint();
        assert_eq!(
            (h.past().len(), h.future().len()),
            (1, 0),
            "a checkpoint drops the redo steps"
        );
    }

    #[test]
    fn history_keeps_the_newest_hundred_steps() {
        let mut h = EditHistory::default();
        h.open(project(0.0));
        for i in 1..=150 {
            h.change(set_speed(i as f64), true);
        }
        assert_eq!(h.past().len(), MAX_HISTORY);
        assert_eq!(h.past()[0].speed, 50.0);
        h.close();
        assert!(h.project().is_none() && h.past().is_empty());
    }
}
