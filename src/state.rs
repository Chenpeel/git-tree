use std::{collections::HashSet, time::Instant};

use crate::{
    graph::Graph,
    repository::{Commit, Details, Snapshot},
    worker::{Update, Worker},
};

pub struct AppState {
    pub worker: Worker,
    pub snapshot: Option<Box<Snapshot>>,
    pub graph: Graph,
    pub selected: Option<String>,
    pub details: Option<Details>,
    pub error: Option<String>,
    pub detail_error: Option<String>,
    pub details_requested: bool,
    pub watch_error: Option<String>,
    pub query: String,
    pub matches: Vec<String>,
    pub match_set: HashSet<String>,
    pub refreshed: Option<Instant>,
    pub revision: usize,
    pub loading_count: Option<usize>,
}

impl AppState {
    pub fn new(worker: Worker) -> Self {
        Self {
            worker,
            snapshot: None,
            graph: Graph::default(),
            selected: None,
            details: None,
            error: None,
            detail_error: None,
            details_requested: false,
            watch_error: None,
            query: String::new(),
            matches: Vec::new(),
            match_set: HashSet::new(),
            refreshed: None,
            revision: 0,
            loading_count: None,
        }
    }

    pub fn poll(&mut self) {
        while let Ok(update) = self.worker.events.try_recv() {
            match update {
                Update::Snapshot(snapshot) => {
                    self.error = None;
                    self.loading_count = None;
                    self.refreshed = Some(Instant::now());
                    self.revision += 1;
                    self.graph = Graph::build(&snapshot);
                    let selected = self
                        .selected
                        .clone()
                        .filter(|id| snapshot.index.contains_key(id))
                        .or_else(|| {
                            snapshot
                                .head
                                .clone()
                                .filter(|id| snapshot.index.contains_key(id))
                        })
                        .or_else(|| snapshot.commits.first().map(|commit| commit.id.clone()));
                    self.snapshot = Some(snapshot);
                    self.update_search();
                    if self.selected != selected {
                        self.selected = selected;
                        self.details = None;
                        self.detail_error = None;
                        self.details_requested = false;
                    }
                }
                Update::Details(details) if self.selected.as_ref() == Some(&details.id) => {
                    self.detail_error = None;
                    self.details = Some(details);
                }
                Update::DetailError { id, error } if self.selected.as_ref() == Some(&id) => {
                    self.detail_error = Some(error);
                    self.details_requested = false;
                }
                Update::Progress(count) => self.loading_count = Some(count),
                Update::LoadError(error) => {
                    self.loading_count = None;
                    self.error = Some(error);
                }
                Update::WatchError(error) => self.watch_error = Some(error),
                _ => {}
            }
        }
    }

    pub fn select(&mut self, id: String) {
        if self.selected.as_ref() != Some(&id) {
            self.selected = Some(id);
            self.details = None;
            self.detail_error = None;
            self.details_requested = false;
        }
        self.request_details();
    }

    pub fn request_details(&mut self) {
        if self.details_requested {
            return;
        }
        if let Some(id) = self.selected.clone() {
            self.details_requested = true;
            self.detail_error = None;
            self.worker.details(id);
        }
    }

    pub fn selected_commit(&self) -> Option<&Commit> {
        self.snapshot.as_ref()?.commit(self.selected.as_deref()?)
    }

    pub fn update_search(&mut self) {
        self.matches = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.search(&self.query))
            .unwrap_or_default();
        self.match_set = self.matches.iter().cloned().collect();
    }

    pub fn step_match(&mut self) {
        let index = self
            .matches
            .iter()
            .position(|id| Some(id) == self.selected.as_ref())
            .map(|index| (index + 1) % self.matches.len().max(1))
            .unwrap_or(0);
        if let Some(id) = self.matches.get(index).cloned() {
            self.select(id);
        }
    }

    pub fn select_head(&mut self) {
        let head = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.head.clone());
        if let Some(id) = head.filter(|id| self.graph.index.contains_key(id)) {
            self.select(id);
        }
    }

    pub fn status(&self) -> String {
        if let Some(error) = &self.error {
            return format!("Read error: {error}");
        }
        if let Some(count) = self.loading_count {
            return format!(
                "Reading commit history... {count} commits read (previous view remains available)"
            );
        }
        let Some(snapshot) = &self.snapshot else {
            return "Reading local commits and references...".to_owned();
        };
        let ago = self
            .refreshed
            .map(|time| time.elapsed().as_secs())
            .unwrap_or(0);
        format!(
            "{} commits | {} refs | trunk: {} | {} | updated {ago}s ago{}{}",
            snapshot.commits.len(),
            snapshot.refs.len(),
            snapshot.trunk_name,
            snapshot.head_branch.as_deref().unwrap_or("detached HEAD"),
            if snapshot.truncated {
                " | HISTORY LIMITED"
            } else {
                ""
            },
            if self.watch_error.is_some() {
                " | WATCH ERROR"
            } else {
                " | LIVE"
            }
        )
    }

    pub fn detail_text(&self) -> String {
        let Some(commit) = self.selected_commit() else {
            return "Select a commit node to inspect its history and changes.".to_owned();
        };
        let mut text = format!(
            "{}\n\nCommit: {}\nAuthor: {} <{}>\nDate: {}\nParents: {}\n\n{}\n\n",
            commit.subject,
            commit.id,
            commit.author,
            commit.email,
            commit.date(),
            if commit.parents.is_empty() {
                "(root)".to_owned()
            } else {
                commit.parents.join("\n         ")
            },
            commit.message
        );
        if let Some(error) = &self.detail_error {
            text.push_str(&format!("Cannot read changes: {error}"));
        } else if let Some(details) = &self.details {
            text.push_str(&format!(
                "{} changed files (vs first parent)\n",
                details.files.len()
            ));
            for file in &details.files {
                text.push_str(&format!("  {:8} {}\n", file.status, file.path));
            }
            text.push_str(&details.diff);
        } else {
            text.push_str(if self.details_requested {
                "Loading changes..."
            } else {
                "Select this commit or press Enter to load file changes."
            });
        }
        text
    }
}
