use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use gix::hash::ObjectId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: ObjectId,
    pub mode: u16,
}

#[derive(Debug)]
pub struct FileDelta {
    pub path: Vec<u8>,
    pub before: Option<Entry>,
    pub after: Option<Entry>,
}

impl FileDelta {
    pub fn status(&self) -> &'static str {
        match (&self.before, &self.after) {
            (None, _) => "added",
            (_, None) => "deleted",
            _ => "modified",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LfsPointer {
    oid: String,
    size: u64,
}

fn lfs_pointer(data: &[u8]) -> Option<LfsPointer> {
    // Git LFS v1 pointer 是小型 UTF-8 文本, 与真实资源是否已经下载无关.
    if data.len() > 1024 {
        return None;
    }
    let text = std::str::from_utf8(data).ok()?;
    let mut lines = text.lines();
    if lines.next()? != "version https://git-lfs.github.com/spec/v1" {
        return None;
    }
    let mut oid = None;
    let mut size = None;
    for line in lines {
        if let Some(hash) = line.strip_prefix("oid sha256:") {
            if oid.is_some()
                || hash.len() != 64
                || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return None;
            }
            oid = Some(hash.to_lowercase());
        } else if let Some(value) = line.strip_prefix("size ") {
            if size.is_some()
                || value.is_empty()
                || !value.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            size = Some(value.parse().ok()?);
        } else if !line.starts_with("ext-") {
            return None;
        }
    }
    Some(LfsPointer {
        oid: oid?,
        size: size?,
    })
}

fn entries(repo: &gix::Repository, id: Option<ObjectId>) -> Result<BTreeMap<Vec<u8>, Entry>> {
    let mut entries = BTreeMap::new();
    if let Some(id) = id {
        let tree = repo.find_tree(id)?;
        for entry in tree.iter() {
            let entry = entry?;
            entries.insert(
                entry.inner.filename.to_vec(),
                Entry {
                    id: entry.inner.oid.to_owned(),
                    mode: entry.inner.mode.value(),
                },
            );
        }
    }
    Ok(entries)
}

pub fn changed_files(
    repo: &gix::Repository,
    before: Option<ObjectId>,
    after: ObjectId,
) -> Result<Vec<FileDelta>> {
    let mut files = Vec::new();
    let mut pending = vec![(Vec::new(), before, Some(after))];
    while let Some((prefix, before, after)) = pending.pop() {
        if before == after {
            continue;
        }
        let old = entries(repo, before)?;
        let new = entries(repo, after)?;
        let names: BTreeSet<_> = old.keys().chain(new.keys()).collect();
        for name in names {
            let old = old.get(name);
            let new = new.get(name);
            if old == new {
                continue;
            }
            let mut path = prefix.clone();
            path.extend_from_slice(name);
            let old_tree = old.is_some_and(|entry| entry.mode & 0o170000 == 0o040000);
            let new_tree = new.is_some_and(|entry| entry.mode & 0o170000 == 0o040000);
            if old_tree || new_tree {
                let mut directory = path.clone();
                directory.push(b'/');
                pending.push((
                    directory,
                    old.filter(|_| old_tree).map(|entry| entry.id),
                    new.filter(|_| new_tree).map(|entry| entry.id),
                ));
            }
            let old = old.filter(|_| !old_tree);
            let new = new.filter(|_| !new_tree);
            if old.is_some() || new.is_some() {
                files.push(FileDelta {
                    path,
                    before: old.cloned(),
                    after: new.cloned(),
                });
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

pub struct Preview {
    pub text: String,
    pub note: Option<String>,
}

impl Preview {
    fn note(note: String) -> Self {
        Self {
            text: String::new(),
            note: Some(note),
        }
    }
}

pub fn preview(repo: &gix::Repository, entry: Option<&Entry>) -> Result<Preview> {
    let Some(entry) = entry else {
        return Ok(Preview {
            text: String::new(),
            note: None,
        });
    };
    if entry.mode & 0o170000 == 0o160000 {
        return Ok(Preview::note(format!("Submodule commit {}", entry.id)));
    }
    let size = repo.find_header(entry.id)?.size();
    if size > 512 * 1024 {
        return Ok(Preview::note(format!(
            "Preview omitted: {size} bytes (512 KiB limit)"
        )));
    }
    let blob = repo.find_blob(entry.id)?;
    if let Some(pointer) = lfs_pointer(&blob.data) {
        return Ok(Preview::note(format!(
            "LFS sha256:{}; declared size: {} bytes (object not downloaded)",
            pointer.oid, pointer.size
        )));
    }
    if blob.data.contains(&0) {
        return Ok(Preview::note(format!("Binary file: {size} bytes")));
    }
    match String::from_utf8(blob.data.clone()) {
        Ok(text) => Ok(Preview { text, note: None }),
        Err(_) => Ok(Preview::note(format!("Non-UTF-8 file: {size} bytes"))),
    }
}

pub fn first_parent_tree(
    repo: &gix::Repository,
    commit: &gix::Commit<'_>,
) -> Result<Option<ObjectId>> {
    commit
        .parent_ids()
        .next()
        .map(|parent| {
            repo.find_commit(parent)
                .context("Cannot compare against missing first parent (shallow history)")?
                .tree_id()
                .map(|id| id.detach())
                .map_err(Into::into)
        })
        .transpose()
}
