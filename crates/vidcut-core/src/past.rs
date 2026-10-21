use std::sync::Arc;

use crate::editor::{Chain, Editor};
use crate::error::Result;
use crate::lru::Lru;
use crate::model::Project;

const KEPT: usize = 32;

#[derive(Debug, Default)]
pub struct Past {
    key: Option<(u64, usize, usize)>,
    states: Lru<usize, Arc<Project>, KEPT>,
    chain: Option<(u64, Arc<Chain>)>,
    applied: usize,
}

impl Past {
    fn fresh(&mut self, editor: &Editor) -> usize {
        let (u, r) = editor.history_counts();
        let key = (editor.project().rev, u, r);
        if self.key != Some(key) {
            self.states = Lru::new();
            self.chain = None;
            self.key = Some(key);
        }
        u
    }

    pub fn version_at(&mut self, editor: &Editor, id: u64, k: usize) -> Result<Arc<Project>> {
        self.fresh(editor);
        let chain = match &self.chain {
            Some((held, c)) if *held == id => c.clone(),
            _ => {
                let c = Arc::new(editor.chain(id)?);
                self.chain = Some((id, c.clone()));
                c
            }
        };
        let start = self.project_at(editor, chain[0].0)?;
        let mut project = (*start).clone();
        crate::editor::climb(&chain, &mut project, k + 1, &mut Vec::new())?;
        Ok(Arc::new(project))
    }

    pub fn project_at(&mut self, editor: &Editor, step: usize) -> Result<Arc<Project>> {
        let u = self.fresh(editor);
        if let Some(project) = self.states.get(&step) {
            return Ok(project.clone());
        }
        let (from, start) = self
            .states
            .iter()
            .map(|(s, p)| (*s, p.as_ref()))
            .chain(std::iter::once((u, editor.project())))
            .min_by_key(|(s, _)| s.abs_diff(step))
            .map(|(s, p)| (s, p.clone()))
            .expect("the current project is always there");
        let project = Arc::new(editor.project_between(start, from, step)?);
        self.applied += from.abs_diff(step);
        self.states.insert(step, project.clone());
        Ok(project)
    }

    pub fn applied(&self) -> usize {
        self.applied
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;
    use crate::model::Settings;
    use crate::request::Envelope;
    use crate::store::Store;
    use crate::test_support::FakeEnv;

    fn store(dir: &Path) -> Store {
        let path = dir.join("p.vidcut");
        let project = Project::new(Settings {
            auto_format: false,
            ..Settings::default()
        });
        std::fs::write(&path, serde_json::to_string(&project).unwrap()).unwrap();
        Store::open_in(&path, &dir.join("state")).unwrap()
    }

    fn run(store: &Store, method: &str, params: serde_json::Value) {
        let e = Envelope::from_json(json!({ "method": method, "params": params })).unwrap();
        store
            .transact(|ed| ed.apply(e, &FakeEnv::default(), None))
            .unwrap();
    }

    fn bins(store: &Store, names: &[&str]) {
        for n in names {
            run(store, "bins.add", json!({ "path": n }));
        }
    }

    #[test]
    fn walking_back_through_the_history_applies_one_step_a_row() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        bins(&s, &["a", "b", "c", "d", "e"]);
        let editor = s.load().unwrap();
        let mut past = Past::default();
        for step in (0..5).rev() {
            past.project_at(&editor, step).unwrap();
        }
        assert_eq!(past.applied(), 5);
    }

    #[test]
    fn a_new_revision_forgets_the_states_it_kept() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        bins(&s, &["a", "b", "c"]);
        let mut past = Past::default();
        past.project_at(&s.load().unwrap(), 2).unwrap();
        run(&s, "history.undo", json!({ "steps": 2 }));
        bins(&s, &["x", "y"]);
        let then = past.project_at(&s.load().unwrap(), 2).unwrap();
        assert_eq!(then.bins, ["a", "x"]);
    }

    #[test]
    fn a_versions_step_shows_the_project_as_it_was_then() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        bins(&s, &["a", "b", "c"]);
        run(&s, "history.undo", json!({ "steps": 2 }));
        bins(&s, &["d"]);
        let then = Past::default()
            .version_at(&s.load().unwrap(), 2, 0)
            .unwrap();
        assert_eq!(then.bins, ["a", "b"]);
    }

    #[test]
    fn a_redo_step_is_made_from_the_nearest_state() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        bins(&s, &["a", "b", "c", "d"]);
        run(&s, "history.undo", json!({ "steps": 4 }));
        let editor = s.load().unwrap();
        let mut past = Past::default();
        for step in 1..=4 {
            past.project_at(&editor, step).unwrap();
        }
        assert_eq!(
            (
                past.applied(),
                past.project_at(&editor, 3).unwrap().bins.clone()
            ),
            (4, vec!["a".to_string(), "b".into(), "c".into()])
        );
    }
}
