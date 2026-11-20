use std::{
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use notify::{EventKind, RecursiveMode, Watcher};

use crate::repository::{self, Details, LoadOptions, Location, Snapshot};

pub enum Command {
    Refresh,
    Changed,
    WatchError(String),
    Details(String),
    Stop,
}

pub enum Update {
    Snapshot(Box<Snapshot>),
    Progress(usize),
    Details(Details),
    LoadError(String),
    DetailError { id: String, error: String },
    WatchError(String),
}

pub struct Worker {
    pub location: Location,
    commands: Sender<Command>,
    pub events: Receiver<Update>,
}

impl Worker {
    pub fn start(location: Location, options: LoadOptions) -> Result<Self> {
        let (commands, receiver) = mpsc::channel();
        let (updates, events) = mpsc::channel();
        let location_in_thread = location.clone();
        let change_sender = commands.clone();
        thread::Builder::new()
            .name("git-tree-reader".to_owned())
            .spawn(move || {
                let mut watcher = match notify::recommended_watcher(
                    move |event: notify::Result<notify::Event>| {
                        let command = match event {
                            Ok(event) if matches!(event.kind, EventKind::Access(_)) => return,
                            Ok(event)
                                if !event.paths.is_empty()
                                    && event.paths.iter().all(|path| {
                                        path.extension()
                                            .is_some_and(|extension| extension == "lock")
                                    }) =>
                            {
                                return;
                            }
                            Ok(_) => Command::Changed,
                            Err(error) => Command::WatchError(error.to_string()),
                        };
                        let _ = change_sender.send(command);
                    },
                ) {
                    Ok(mut watcher) => {
                        let mut paths = vec![&location_in_thread.common_dir];
                        if !location_in_thread
                            .git_dir
                            .starts_with(&location_in_thread.common_dir)
                        {
                            paths.push(&location_in_thread.git_dir);
                        }
                        for path in paths {
                            if let Err(error) = watcher.watch(path, RecursiveMode::Recursive) {
                                let _ = updates.send(Update::WatchError(format!(
                                    "{}: {error}. Use manual refresh.",
                                    path.display()
                                )));
                            }
                        }
                        Some(watcher)
                    }
                    Err(error) => {
                        let _ = updates
                            .send(Update::WatchError(format!("{error}. Use manual refresh.")));
                        None
                    }
                };
                let mut reload_at = Some(Instant::now());
                let mut first_change = None;
                let mut pending_details = None;
                'reader: loop {
                    let timeout = if pending_details.is_some() {
                        Duration::ZERO
                    } else {
                        reload_at
                            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                            .unwrap_or(Duration::from_secs(60))
                    };
                    let command = match receiver.recv_timeout(timeout) {
                        Ok(command) => Some(command),
                        Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => None,
                    };
                    // 合并突发文件事件和快速点击, 只读取最新选择的详情.
                    for command in command.into_iter().chain(receiver.try_iter()) {
                        match command {
                            Command::Stop => break 'reader,
                            Command::Refresh => {
                                first_change = None;
                                reload_at = Some(Instant::now());
                            }
                            Command::Changed => {
                                let now = Instant::now();
                                let first = *first_change.get_or_insert(now);
                                reload_at = Some(
                                    (now + Duration::from_millis(220))
                                        .min(first + Duration::from_secs(1)),
                                );
                            }
                            Command::WatchError(error) => {
                                let _ = updates.send(Update::WatchError(error));
                            }
                            Command::Details(id) => pending_details = Some(id),
                        }
                    }
                    if reload_at.is_some_and(|deadline| Instant::now() >= deadline) {
                        reload_at = None;
                        first_change = None;
                        let update =
                            match repository::load(&location_in_thread, &options, |count| {
                                let _ = updates.send(Update::Progress(count));
                            }) {
                                Ok(snapshot) => Update::Snapshot(Box::new(snapshot)),
                                Err(error) => Update::LoadError(format!("{error:#}")),
                            };
                        if updates.send(update).is_err() {
                            break;
                        }
                    }
                    if let Some(id) = pending_details.take() {
                        let update = match repository::details(&location_in_thread, &id) {
                            Ok(details) => Update::Details(details),
                            Err(error) => Update::DetailError {
                                id,
                                error: format!("{error:#}"),
                            },
                        };
                        if updates.send(update).is_err() {
                            break;
                        }
                    }
                }
                // 在读取线程退出前释放监听器, 不让平台回调持有通道.
                drop(watcher.take());
            })
            .context("Cannot start repository reader")?;
        Ok(Self {
            location,
            commands,
            events,
        })
    }

    pub fn refresh(&self) {
        let _ = self.commands.send(Command::Refresh);
    }

    pub fn details(&self, id: String) {
        let _ = self.commands.send(Command::Details(id));
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
    }
}
