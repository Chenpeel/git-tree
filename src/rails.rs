use std::{
    cmp::Reverse,
    collections::{BTreeSet, BinaryHeap, HashMap, HashSet},
};

use crate::repository::Snapshot;

#[derive(Debug, Clone)]
pub struct RailEdge {
    pub child: usize,
    pub parent: usize,
    pub from_lane: usize,
    pub route_lane: usize,
    pub to_lane: usize,
    pub merge: bool,
}

#[derive(Debug, Default)]
pub struct RailLayout {
    pub lanes: Vec<usize>,
    pub edges: Vec<RailEdge>,
    pub width: usize,
    pub parents: Vec<Vec<usize>>,
    pub trunk: HashSet<usize>,
}

fn allocate(free: &mut BTreeSet<usize>, next: &mut usize) -> usize {
    if let Some(lane) = free.pop_first() {
        lane
    } else {
        let lane = *next;
        *next += 1;
        lane
    }
}

impl RailLayout {
    pub fn build(snapshot: &Snapshot) -> Self {
        let count = snapshot.commits.len();
        let parents: Vec<Vec<usize>> = snapshot
            .commits
            .iter()
            .map(|commit| {
                commit
                    .parents
                    .iter()
                    .filter_map(|id| snapshot.index.get(id).copied())
                    .collect()
            })
            .collect();
        let mut trunk = HashSet::new();
        let mut cursor = snapshot
            .trunk
            .as_deref()
            .and_then(|id| snapshot.index.get(id))
            .copied();
        while let Some(index) = cursor {
            if !trunk.insert(index) {
                break;
            }
            cursor = snapshot.commits[index]
                .parents
                .first()
                .and_then(|id| snapshot.index.get(id))
                .copied();
        }
        let mut lanes = vec![0; count];
        let mut waiting = HashMap::<usize, usize>::new();
        let mut edges = Vec::new();
        let mut free = BTreeSet::new();
        let mut next = 1;
        let mut width = usize::from(count > 0);
        for (child, node_lane) in lanes.iter_mut().enumerate() {
            // waiting 只含尚未处理的父提交. 已结束的侧通道可在下一行复用.
            let lane = if let Some(lane) = waiting.remove(&child) {
                lane
            } else if trunk.contains(&child) {
                0
            } else {
                allocate(&mut free, &mut next)
            };
            *node_lane = lane;
            width = width.max(lane + 1);
            let mut continued = false;
            let mut deferred_free = Vec::new();
            for (number, id) in snapshot.commits[child].parents.iter().enumerate() {
                let Some(&parent) = snapshot.index.get(id) else {
                    continue;
                };
                let route_lane = if let Some(&existing) = waiting.get(&parent) {
                    existing
                } else if trunk.contains(&parent) {
                    waiting.insert(parent, 0);
                    0
                } else if number == 0 && lane != 0 {
                    continued = true;
                    waiting.insert(parent, lane);
                    lane
                } else {
                    let route = allocate(&mut free, &mut next);
                    waiting.insert(parent, route);
                    route
                };
                // 入同一父提交的边共享其预约轨道, 不为每个历史分支永久占列.
                if route_lane == lane {
                    continued = true;
                }
                width = width.max(route_lane + 1);
                edges.push(RailEdge {
                    child,
                    parent,
                    from_lane: lane,
                    route_lane,
                    to_lane: route_lane,
                    merge: number > 0,
                });
            }
            if lane != 0 && !continued {
                deferred_free.push(lane);
            }
            for lane in deferred_free {
                free.insert(lane);
            }
        }
        let mut node_rows = vec![Vec::new(); width];
        for (row, &lane) in lanes.iter().enumerate() {
            node_rows[lane].push(row);
        }
        let mut detours = BinaryHeap::<Reverse<(usize, usize)>>::new();
        let mut free_detours = BTreeSet::new();
        let mut next_detour = width;
        for edge in &mut edges {
            edge.to_lane = lanes[edge.parent];
            while let Some(&Reverse((end, lane))) = detours.peek() {
                if end >= edge.child {
                    break;
                }
                detours.pop();
                free_detours.insert(lane);
            }
            let rows = &node_rows[edge.route_lane];
            let next_node = rows.partition_point(|&row| row <= edge.child);
            if rows.get(next_node).is_some_and(|&row| row < edge.parent) {
                // 远距合并不要穿过不相干的主干节点, 改走可复用的旁路.
                edge.route_lane = allocate(&mut free_detours, &mut next_detour);
                detours.push(Reverse((edge.parent, edge.route_lane)));
            }
        }
        width = width.max(next_detour);
        Self {
            lanes,
            edges,
            width,
            parents,
            trunk,
        }
    }

    pub fn ancestry(&self, index: usize) -> HashSet<usize> {
        let mut result = HashSet::new();
        let mut pending = vec![index];
        while let Some(index) = pending.pop() {
            if result.insert(index) {
                pending.extend(self.parents[index].iter().copied());
            }
        }
        result
    }
}
