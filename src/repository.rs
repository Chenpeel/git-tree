use std::{
    collections::{BinaryHeap, HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use gix::{bstr::ByteSlice, hash::ObjectId};
use similar::TextDiff;

use crate::changes;

#[derive(Debug, Clone)]
pub struct Location {
    pub path: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub title: String,
}

impl Location {
    pub fn discover(path: &Path) -> Result<Self> {
        let repo = gix::discover_opts(path, Default::default(), gix::open::Options::isolated())
            .with_context(|| format!("Cannot discover a Git repository at {}", path.display()))?;
        let git_dir = repo.git_dir().canonicalize()?;
        let common_dir = repo.common_dir().canonicalize()?;
        let path = repo.workdir().unwrap_or(repo.git_dir()).canonicalize()?;
        let title = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
        Ok(Self {
            path,
            git_dir,
            common_dir,
            title,
        })
    }

    fn open(&self) -> Result<gix::Repository> {
        gix::open_opts(&self.git_dir, gix::open::Options::isolated())
            .context("Cannot open repository metadata")
    }
}

#[derive(Debug, Clone)]
pub struct LoadOptions {
    pub main_branch: Option<String>,
    pub max_commits: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Commit {
    pub id: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub message: String,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub offset: i32,
}

impl Commit {
    pub fn short_id(&self) -> &str {
        &self.id[..self.id.len().min(8)]
    }

    pub fn date(&self) -> String {
        chrono::DateTime::from_timestamp(self.time, 0)
            .and_then(|date| {
                chrono::FixedOffset::east_opt(self.offset).map(|offset| date.with_timezone(&offset))
            })
            .map(|date| date.format("%Y-%m-%d %H:%M:%S %:z").to_string())
            .unwrap_or_else(|| self.time.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    Branch,
    Remote,
    Tag,
}

#[derive(Debug, Clone)]
pub struct Reference {
    pub name: String,
    pub full_name: String,
    pub target: String,
    pub kind: RefKind,
}

#[derive(Debug)]
pub struct Snapshot {
    pub commits: Vec<Commit>,
    pub index: HashMap<String, usize>,
    pub refs: Vec<Reference>,
    labels: HashMap<String, Vec<usize>>,
    pub head: Option<String>,
    pub head_branch: Option<String>,
    pub trunk: Option<String>,
    pub trunk_name: String,
    pub warnings: Vec<String>,
    pub truncated: bool,
}

impl Snapshot {
    pub fn commit(&self, id: &str) -> Option<&Commit> {
        self.index.get(id).map(|&index| &self.commits[index])
    }

    pub fn labels(&self, id: &str) -> Vec<&str> {
        self.labels
            .get(id)
            .into_iter()
            .flatten()
            .map(|&index| self.refs[index].name.as_str())
            .collect()
    }

    pub fn search(&self, query: &str) -> Vec<String> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let ref_targets: HashSet<_> = self
            .refs
            .iter()
            .filter(|reference| reference.name.to_lowercase().contains(&query))
            .map(|reference| reference.target.as_str())
            .collect();
        self.commits
            .iter()
            .filter(|commit| {
                commit.id.starts_with(&query)
                    || commit.subject.to_lowercase().contains(&query)
                    || commit.author.to_lowercase().contains(&query)
                    || ref_targets.contains(commit.id.as_str())
            })
            .map(|commit| commit.id.clone())
            .collect()
    }
}

fn decode_commit(commit: &gix::Commit<'_>) -> Result<Commit> {
    let author = commit.author()?;
    let time = author.time()?;
    let message = commit.message_raw()?.to_str_lossy().into_owned();
    Ok(Commit {
        id: commit.id.to_string(),
        parents: commit.parent_ids().map(|id| id.to_string()).collect(),
        subject: message.lines().next().unwrap_or("(no subject)").to_owned(),
        message,
        author: author.name.to_str_lossy().into_owned(),
        email: author.email.to_str_lossy().into_owned(),
        time: time.seconds,
        offset: time.offset,
    })
}

pub fn load(
    location: &Location,
    options: &LoadOptions,
    mut progress: impl FnMut(usize),
) -> Result<Snapshot> {
    let repo = location.open()?;
    let head_ref = repo.head()?;
    let head_branch = head_ref.referent_name().map(|name| {
        name.as_bstr()
            .to_str_lossy()
            .trim_start_matches("refs/heads/")
            .to_owned()
    });
    let head = if head_ref.is_unborn() {
        None
    } else {
        Some(repo.head_commit()?.id.to_string())
    };
    let mut warnings = Vec::new();
    let mut refs = Vec::new();
    {
        let references = {
            let platform = repo.references()?;
            platform.all()?.collect::<gix::Result<Vec<_>>>()?
        };
        for mut reference in references {
            let full_name = reference.name().as_bstr().to_str_lossy().into_owned();
            let (kind, name) = if let Some(name) = full_name.strip_prefix("refs/heads/") {
                (RefKind::Branch, name.to_owned())
            } else if let Some(name) = full_name.strip_prefix("refs/remotes/") {
                (RefKind::Remote, name.to_owned())
            } else if let Some(name) = full_name.strip_prefix("refs/tags/") {
                (RefKind::Tag, format!("tag:{name}"))
            } else {
                continue;
            };
            let id = reference.peel_to_id()?;
            if repo.find_header(id)?.kind() != gix::objs::Kind::Commit {
                warnings.push(format!("Non-commit reference omitted: {full_name}"));
                continue;
            }
            refs.push(Reference {
                name,
                full_name,
                target: id.to_string(),
                kind,
            });
        }
    }
    refs.sort_by(|a, b| (&a.kind, &a.name).cmp(&(&b.kind, &b.name)));

    let trunk_ref = if let Some(name) = &options.main_branch {
        let reference = refs
            .iter()
            .find(|reference| reference.full_name == *name || reference.name == *name);
        if reference.is_none() {
            bail!("Main branch '{name}' does not exist. Use an existing branch or full ref.");
        }
        reference
    } else {
        ["main", "master"]
            .into_iter()
            .find_map(|name| {
                refs.iter()
                    .find(|reference| reference.kind == RefKind::Branch && reference.name == name)
            })
            .or_else(|| {
                refs.iter().find(|reference| {
                    reference.kind == RefKind::Branch
                        && Some(&reference.name) == head_branch.as_ref()
                })
            })
            .or_else(|| refs.iter().find(|reference| reference.kind != RefKind::Tag))
            .or_else(|| refs.first())
    };
    let trunk = trunk_ref
        .map(|reference| reference.target.clone())
        .or_else(|| head.clone());
    let trunk_name = trunk_ref
        .map(|reference| reference.name.clone())
        .or_else(|| head_branch.clone())
        .unwrap_or_else(|| "HEAD".to_owned());

    let shallow_path = location.common_dir.join("shallow");
    let shallow: HashSet<String> = if shallow_path.exists() {
        std::fs::read_to_string(&shallow_path)?
            .lines()
            .map(str::to_owned)
            .collect()
    } else {
        HashSet::new()
    };
    if !shallow.is_empty() {
        warnings.push(
            "Shallow repository: roots may be history boundaries, not original commits.".to_owned(),
        );
    }
    let mut pending: VecDeque<String> = trunk.iter().chain(head.iter()).cloned().collect();
    pending.extend(refs.iter().map(|reference| reference.target.clone()));
    let mut seen = HashSet::new();
    let mut commits = HashMap::new();
    let mut truncated = false;
    // 使用所有引用作为遍历起点, 避免只显示当前分支而遗漏旁支.
    while let Some(id) = pending.pop_front() {
        if seen.contains(&id) {
            continue;
        }
        if options
            .max_commits
            .is_some_and(|limit| commits.len() >= limit)
        {
            truncated = true;
            break;
        }
        seen.insert(id.clone());
        let oid = ObjectId::from_hex(id.as_bytes())?;
        let object = repo
            .find_commit(oid)
            .with_context(|| format!("Cannot read commit {id}"))?;
        let commit = decode_commit(&object)?;
        if !shallow.contains(&id) {
            pending.extend(commit.parents.iter().cloned());
        }
        commits.insert(id, commit);
        if commits.len() % 100 == 0 {
            progress(commits.len());
        }
    }
    let commits = topological_order(commits)?;
    let index = commits
        .iter()
        .enumerate()
        .map(|(index, commit)| (commit.id.clone(), index))
        .collect();
    let mut labels = HashMap::<String, Vec<usize>>::new();
    for (index, reference) in refs.iter().enumerate() {
        labels
            .entry(reference.target.clone())
            .or_default()
            .push(index);
    }
    Ok(Snapshot {
        commits,
        index,
        refs,
        labels,
        head,
        head_branch,
        trunk,
        trunk_name,
        warnings,
        truncated,
    })
}

fn topological_order(mut commits: HashMap<String, Commit>) -> Result<Vec<Commit>> {
    let mut children: HashMap<String, usize> = commits.keys().map(|id| (id.clone(), 0)).collect();
    for commit in commits.values() {
        for parent in &commit.parents {
            if let Some(count) = children.get_mut(parent) {
                *count += 1;
            }
        }
    }
    let mut ready = BinaryHeap::new();
    for (id, count) in &children {
        if *count == 0 {
            ready.push((commits[id].time, id.clone()));
        }
    }
    let mut sorted = Vec::with_capacity(commits.len());
    while let Some((_, id)) = ready.pop() {
        let commit = commits.remove(&id).context("Invalid history graph")?;
        for parent in &commit.parents {
            if let Some(count) = children.get_mut(parent) {
                *count -= 1;
                if *count == 0 {
                    ready.push((commits[parent].time, parent.clone()));
                }
            }
        }
        sorted.push(commit);
    }
    anyhow::ensure!(commits.is_empty(), "Invalid cyclic commit history");
    Ok(sorted)
}

#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: String,
    pub status: &'static str,
}

#[derive(Debug)]
pub struct Details {
    pub id: String,
    pub files: Vec<FileChange>,
    pub diff: String,
    #[cfg(feature = "gui")]
    pub diff_lines: Vec<String>,
}

pub fn details(location: &Location, id: &str) -> Result<Details> {
    let repo = location.open()?;
    let oid = ObjectId::from_hex(id.as_bytes())?;
    let commit = repo.find_commit(oid)?;
    let previous = changes::first_parent_tree(&repo, &commit)?;
    let changed = changes::changed_files(&repo, previous, commit.tree_id()?.detach())?;
    let mut files = Vec::with_capacity(changed.len());
    let mut diff = String::new();
    let mut preview_limited = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;
    for file in changed {
        let path = String::from_utf8_lossy(&file.path).into_owned();
        files.push(FileChange {
            path: path.clone(),
            status: file.status(),
        });
        if preview_limited {
            continue;
        }
        if diff.len() >= MAX_DIFF_BYTES || std::time::Instant::now() >= deadline {
            diff.push_str(
                "[Diff preview limited to 2 MiB / 3 seconds. All changed files remain listed.]\n",
            );
            preview_limited = true;
            continue;
        }
        diff.push_str(&format!("\ndiff --git a/{path} b/{path}\n"));
        if let (Some(old), Some(new)) = (&file.before, &file.after)
            && old.id == new.id
            && old.mode & 0o170000 == new.mode & 0o170000
        {
            diff.push_str(&format!("Mode: {:06o} -> {:06o}\n", old.mode, new.mode));
            continue;
        }
        match (
            changes::preview(&repo, file.before.as_ref()),
            changes::preview(&repo, file.after.as_ref()),
        ) {
            (Ok(old), Ok(new)) => {
                if let Some(note) = &old.note {
                    diff.push_str(&format!("Before: {note}\n"));
                }
                if let Some(note) = &new.note {
                    diff.push_str(&format!("After: {note}\n"));
                }
                if old.note.is_some() || new.note.is_some() {
                    diff.push_str(
                        "Text diff omitted for non-text or unavailable preview content.\n",
                    );
                    continue;
                }
                if old.text.is_empty() && new.text.is_empty() {
                    continue;
                }
                let text_diff = TextDiff::configure()
                    .timeout(Duration::from_millis(150))
                    .diff_lines(&old.text, &new.text);
                let unified = text_diff
                    .unified_diff()
                    .context_radius(3)
                    .header(
                        if file.before.is_none() {
                            "/dev/null"
                        } else {
                            &path
                        },
                        if file.after.is_none() {
                            "/dev/null"
                        } else {
                            &path
                        },
                    )
                    .to_string();
                if diff.len() + unified.len() > MAX_DIFF_BYTES {
                    diff.push_str("[Diff preview limited to 2 MiB.]\n");
                    preview_limited = true;
                } else {
                    diff.push_str(&unified);
                }
            }
            (Err(error), _) | (_, Err(error)) => diff.push_str(&format!("{error:#}\n")),
        }
    }
    if files.is_empty() {
        diff.push_str("No file changes compared with the first parent.\n");
    }
    #[cfg(feature = "gui")]
    let diff_lines = diff.lines().map(str::to_owned).collect();
    Ok(Details {
        id: id.to_owned(),
        files,
        diff,
        #[cfg(feature = "gui")]
        diff_lines,
    })
}
