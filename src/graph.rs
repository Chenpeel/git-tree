use std::collections::{HashMap, HashSet};

use crate::{
    rails::RailLayout,
    repository::{RefKind, Snapshot},
};

#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    #[cfg(feature = "gui")]
    pub lane: i32,
    pub display_row: usize,
    #[cfg(feature = "gui")]
    pub trunk: bool,
    pub boundary: bool,
}

#[cfg(feature = "gui")]
#[derive(Debug, Clone)]
pub struct Edge {
    pub child: usize,
    pub parent: usize,
    pub merge: bool,
}

#[derive(Debug, Default)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub index: HashMap<String, usize>,
    #[cfg(feature = "gui")]
    pub edges: Vec<Edge>,
    pub rails: RailLayout,
}

impl Graph {
    pub fn build(snapshot: &Snapshot) -> Self {
        let mut lanes = HashMap::<String, i32>::new();
        let mut trunk = HashSet::new();
        let mut cursor = snapshot.trunk.clone();
        while let Some(id) = cursor {
            let Some(commit) = snapshot.commit(&id) else {
                break;
            };
            if !trunk.insert(id.clone()) {
                break;
            }
            lanes.insert(id, 0);
            cursor = commit.parents.first().cloned();
        }
        let mut tips: Vec<_> = snapshot
            .refs
            .iter()
            .filter(|reference| reference.kind != RefKind::Tag)
            .map(|reference| reference.target.as_str())
            .collect();
        tips.extend(snapshot.commits.iter().map(|commit| commit.id.as_str()));
        let mut branch_number = 0_i32;
        // 每条第一父链共享一个侧向位置, 所有其他父连接仍独立绘制为合并边.
        for tip in tips {
            if lanes.contains_key(tip) {
                continue;
            }
            branch_number += 1;
            let distance = (branch_number + 1) / 2;
            let lane = if branch_number % 2 == 1 {
                -distance
            } else {
                distance
            };
            let mut cursor = Some(tip);
            while let Some(id) = cursor {
                if lanes.contains_key(id) {
                    break;
                }
                let Some(commit) = snapshot.commit(id) else {
                    break;
                };
                lanes.insert(id.to_owned(), lane);
                cursor = commit.parents.first().map(String::as_str);
            }
        }
        let nodes: Vec<_> = snapshot
            .commits
            .iter()
            .enumerate()
            .map(|(row, commit)| Node {
                id: commit.id.clone(),
                #[cfg(feature = "gui")]
                lane: *lanes.get(&commit.id).unwrap_or(&0),
                display_row: row * 2,
                #[cfg(feature = "gui")]
                trunk: trunk.contains(&commit.id),
                boundary: commit
                    .parents
                    .iter()
                    .any(|parent| !snapshot.index.contains_key(parent)),
            })
            .collect();
        #[cfg(feature = "gui")]
        let mut edges = Vec::new();
        #[cfg(feature = "gui")]
        for (child, commit) in snapshot.commits.iter().enumerate() {
            for (number, parent) in commit.parents.iter().enumerate() {
                if let Some(&parent) = snapshot.index.get(parent) {
                    edges.push(Edge {
                        child,
                        parent,
                        merge: number > 0,
                    });
                }
            }
        }
        Self {
            nodes,
            index: snapshot.index.clone(),
            #[cfg(feature = "gui")]
            edges,
            rails: RailLayout::build(snapshot),
        }
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.index.get(id).map(|&index| &self.nodes[index])
    }
}

pub fn lane_rgb(lane: i32) -> [u8; 3] {
    const PALETTE: [[u8; 3]; 8] = [
        [103, 232, 170],
        [129, 181, 255],
        [231, 176, 255],
        [248, 198, 112],
        [104, 215, 228],
        [255, 145, 166],
        [176, 205, 119],
        [183, 164, 255],
    ];
    let index = if lane < 0 {
        lane.unsigned_abs() as usize * 2 - 1
    } else {
        lane as usize * 2
    };
    PALETTE[index % PALETTE.len()]
}
