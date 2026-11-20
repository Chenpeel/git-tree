use std::collections::{HashMap, HashSet};

use eframe::egui::{Pos2, Rect, Vec2};
use glam::{Mat4, Vec3};

use crate::{
    graph::{Graph, lane_rgb},
    repository::Snapshot,
};

const FOV: f32 = 45.0_f32.to_radians();

pub struct SceneNode {
    pub position: Vec3,
    pub radius: f32,
    pub color: [f32; 3],
    pub important: bool,
}

pub struct Branch {
    pub from: Vec3,
    pub to: Vec3,
    pub radius: f32,
    pub color: [f32; 3],
    pub child: usize,
    pub parent: usize,
    pub merge: bool,
    pub trunk: bool,
}

#[derive(Default)]
pub struct Scene {
    pub nodes: Vec<SceneNode>,
    pub branches: Vec<Branch>,
    pub min: Vec3,
    pub max: Vec3,
}

struct Route {
    anchor: Vec3,
    base: f32,
    top: f32,
    direction: Vec3,
    reach: f32,
}

impl Scene {
    pub fn build(graph: &Graph, snapshot: &Snapshot) -> Self {
        let count = graph.nodes.len();
        if count == 0 {
            return Self::default();
        }
        let first: Vec<_> = snapshot
            .commits
            .iter()
            .map(|commit| {
                commit
                    .parents
                    .first()
                    .and_then(|id| graph.index.get(id).copied())
            })
            .collect();
        let mut generations = vec![0_u32; count];
        let mut children = vec![0_usize; count];
        for index in (0..count).rev() {
            generations[index] = snapshot.commits[index]
                .parents
                .iter()
                .filter_map(|id| graph.index.get(id))
                .map(|&parent| generations[parent] + 1)
                .max()
                .unwrap_or(0);
        }
        for edge in &graph.edges {
            children[edge.parent] += 1;
        }
        let tips: HashSet<_> = snapshot
            .refs
            .iter()
            .filter_map(|reference| graph.index.get(&reference.target))
            .copied()
            .collect();
        let merge_ends: HashSet<_> = graph
            .edges
            .iter()
            .filter(|edge| edge.merge)
            .flat_map(|edge| [edge.child, edge.parent])
            .collect();
        let depth = generations.iter().copied().max().unwrap_or(0).max(1) as f32;
        let height = 16.0 + (count as f32 + 1.0).log2() * 1.6;
        let spread = height * 0.46;
        let mut lane_top = HashMap::<i32, f32>::new();
        for (index, node) in graph.nodes.iter().enumerate() {
            lane_top
                .entry(node.lane)
                .and_modify(|top| *top = top.max(generations[index] as f32))
                .or_insert(generations[index] as f32);
        }
        let mut routes = HashMap::<i32, Route>::new();
        let mut forks = HashMap::<Option<usize>, u32>::new();
        let mut positions = vec![Vec3::ZERO; count];
        for index in (0..count).rev() {
            let node = &graph.nodes[index];
            let generation = generations[index] as f32;
            let y = height * (generation / depth).powf(0.86);
            if node.trunk {
                positions[index] = Vec3::new(0.0, y, 0.0);
                continue;
            }
            let route = routes.entry(node.lane).or_insert_with(|| {
                let base = first[index]
                    .map(|parent| generations[parent] as f32)
                    .unwrap_or(generation - 1.0);
                let order = forks.entry(first[index]).or_default();
                let angle = first[index]
                    .map(|parent| stable_angle(&graph.nodes[parent].id))
                    .unwrap_or_else(|| stable_angle(&node.id))
                    + *order as f32 * 2.399_963_1;
                *order += 1;
                let anchor = first[index]
                    .map(|parent| positions[parent])
                    .unwrap_or_else(|| {
                        Vec3::new(
                            angle.cos() * spread * 0.65,
                            0.0,
                            angle.sin() * spread * 0.65,
                        )
                    });
                let outward = Vec3::new(anchor.x, 0.0, anchor.z).normalize_or_zero();
                let direction =
                    (Vec3::new(angle.cos(), 0.0, angle.sin()) + outward * 1.25).normalize();
                Route {
                    anchor,
                    base,
                    top: lane_top[&node.lane],
                    direction,
                    reach: spread
                        * ((lane_top[&node.lane] - base) / depth)
                            .sqrt()
                            .clamp(0.2, 1.0),
                }
            });
            let progress =
                ((generation - route.base) / (route.top - route.base).max(1.0)).clamp(0.0, 1.0);
            let offset = route.direction * route.reach * progress.powf(0.72);
            positions[index] = Vec3::new(route.anchor.x + offset.x, y, route.anchor.z + offset.z);
        }
        let mut nodes: Vec<_> = graph
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| SceneNode {
                position: positions[index],
                radius: if node.trunk { 0.15 } else { 0.12 },
                color: lane_rgb(node.lane).map(|channel| f32::from(channel) / 255.0),
                important: tips.contains(&index)
                    || merge_ends.contains(&index)
                    || children[index] != 1
                    || snapshot.commits[index].parents.len() != 1
                    || node.boundary
                    || first[index].is_some_and(|parent| graph.nodes[parent].lane != node.lane),
            })
            .collect();
        for edge in &graph.edges {
            let radius =
                (positions[edge.child].distance(positions[edge.parent]) * 0.25).max(0.00001);
            nodes[edge.child].radius = nodes[edge.child].radius.min(radius);
            nodes[edge.parent].radius = nodes[edge.parent].radius.min(radius);
        }
        let mut branches = Vec::new();
        for edge in &graph.edges {
            let child = &nodes[edge.child];
            let parent = &nodes[edge.parent];
            let trunk = graph.nodes[edge.child].trunk && graph.nodes[edge.parent].trunk;
            let points = connection_curve(child.position, parent.position, edge.merge);
            let radius = child.radius.min(parent.radius)
                * if edge.merge {
                    0.12
                } else if trunk {
                    0.30
                } else {
                    0.22
                };
            let color = if edge.merge {
                parent.color
            } else {
                child.color
            };
            for pair in points.windows(2) {
                branches.push(Branch {
                    from: pair[0],
                    to: pair[1],
                    radius,
                    color,
                    child: edge.child,
                    parent: edge.parent,
                    merge: edge.merge,
                    trunk,
                });
            }
        }
        let mut min = nodes[0].position;
        let mut max = min;
        for node in &nodes {
            min = min.min(node.position);
            max = max.max(node.position);
        }
        for edge in &branches {
            min = min.min(edge.from).min(edge.to);
            max = max.max(edge.from).max(edge.to);
        }
        Self {
            nodes,
            branches,
            min: min - Vec3::ONE,
            max: max + Vec3::ONE,
        }
    }

    pub fn radius(&self) -> f32 {
        (self.max - self.min).length().max(4.0) * 0.5
    }

    pub fn occluded(&self, origin: Vec3, direction: Vec3, distance: f32, pixel_scale: f32) -> bool {
        self.branches.iter().any(|edge| {
            let delta = edge.to - edge.from;
            let along = ((origin - edge.from).dot(delta) / delta.length_squared().max(1e-12))
                .clamp(0.0, 1.0);
            let radius = edge
                .radius
                .max(origin.distance(edge.from + delta * along) * pixel_scale * 0.5);
            ray_capsule(origin, direction, edge.from, edge.to, radius)
                .is_some_and(|hit| hit > 0.0 && hit < distance - 0.0001)
        })
    }
}

fn stable_angle(id: &str) -> f32 {
    let hash = id.bytes().fold(2_166_136_261_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    });
    hash as f32 / u32::MAX as f32 * std::f32::consts::TAU
}

fn connection_curve(from: Vec3, to: Vec3, merge: bool) -> Vec<Vec3> {
    if (from.x - to.x).abs() + (from.z - to.z).abs() < 0.0001 {
        return vec![from, to];
    }
    let delta = from - to;
    let rise = delta.y.abs().max(0.05);
    let sideways = if merge {
        Vec3::new(-delta.z, 0.0, delta.x).normalize_or_zero() * delta.length().min(4.0) * 0.16
    } else {
        Vec3::ZERO
    };
    let a = from - Vec3::Y * rise * 0.45 + sideways;
    let b = to + Vec3::Y * rise * 0.45 + sideways;
    (0..=10)
        .map(|step| {
            let t = step as f32 / 10.0;
            let u = 1.0 - t;
            from * u.powi(3) + a * (3.0 * u * u * t) + b * (3.0 * u * t * t) + to * t.powi(3)
        })
        .collect()
}

pub struct Camera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.65,
            pitch: 0.14,
            distance: 35.0,
        }
    }
}

impl Camera {
    pub fn eye(&self) -> Vec3 {
        self.target
            + Vec3::new(
                self.yaw.sin() * self.pitch.cos(),
                self.pitch.sin(),
                self.yaw.cos() * self.pitch.cos(),
            ) * self.distance
    }

    pub fn matrix(&self, rect: Rect, radius: f32) -> Mat4 {
        let near = (self.distance - radius * 2.5)
            .max(self.distance * 0.001)
            .max(0.0001);
        let far = self.distance + radius * 4.0 + 20.0;
        Mat4::perspective_rh_gl(
            FOV,
            (rect.width() / rect.height().max(1.0)).max(0.01),
            near,
            far,
        ) * Mat4::look_at_rh(self.eye(), self.target, Vec3::Y)
    }

    pub fn fit(&mut self, scene: &Scene, rect: Rect) {
        self.target = (scene.min + scene.max) * 0.5;
        let vertical = FOV * 0.5;
        let horizontal = (vertical.tan() * rect.width() / rect.height().max(1.0)).atan();
        self.distance = scene.radius() / vertical.min(horizontal).max(0.05).sin() * 1.12;
    }

    pub fn orbit(&mut self, delta: Vec2) {
        self.yaw = (self.yaw - delta.x * 0.006).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + delta.y * 0.006).clamp(-1.25, 1.25);
    }

    pub fn pan(&mut self, delta: Vec2, rect: Rect) {
        let back = (self.eye() - self.target).normalize();
        let right = Vec3::Y.cross(back).normalize();
        let up = back.cross(right);
        self.target += (-right * delta.x + up * delta.y) * self.units_per_point(rect);
    }

    pub fn units_per_point(&self, rect: Rect) -> f32 {
        2.0 * self.distance * (FOV * 0.5).tan() / rect.height().max(1.0)
    }

    pub fn project(matrix: Mat4, rect: Rect, position: Vec3) -> Option<(Pos2, f32)> {
        let clip = matrix * position.extend(1.0);
        if clip.w <= 0.0 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        if !(-1.0..=1.0).contains(&ndc.z) {
            return None;
        }
        let point =
            rect.center() + Vec2::new(ndc.x * rect.width() * 0.5, -ndc.y * rect.height() * 0.5);
        rect.expand(16.0).contains(point).then_some((point, clip.w))
    }

    pub fn ray(&self, matrix: Mat4, rect: Rect, pointer: Pos2) -> (Vec3, Vec3) {
        let ndc = Vec3::new(
            (pointer.x - rect.center().x) * 2.0 / rect.width().max(1.0),
            (rect.center().y - pointer.y) * 2.0 / rect.height().max(1.0),
            1.0,
        );
        let far = matrix.inverse().project_point3(ndc);
        (self.eye(), (far - self.eye()).normalize())
    }
}

pub fn ray_sphere(origin: Vec3, direction: Vec3, center: Vec3, radius: f32) -> Option<f32> {
    let offset = origin - center;
    let b = offset.dot(direction);
    let discriminant = b * b - offset.length_squared() + radius * radius;
    if discriminant < 0.0 {
        return None;
    }
    let near = -b - discriminant.sqrt();
    let far = -b + discriminant.sqrt();
    if near >= 0.0 {
        Some(near)
    } else {
        (far >= 0.0).then_some(far)
    }
}

fn ray_capsule(origin: Vec3, direction: Vec3, from: Vec3, to: Vec3, radius: f32) -> Option<f32> {
    let ba = to - from;
    let oa = origin - from;
    let baba = ba.length_squared();
    let bard = ba.dot(direction);
    let baoa = ba.dot(oa);
    let a = baba - bard * bard;
    let b = baba * direction.dot(oa) - baoa * bard;
    let c = baba * oa.length_squared() - baoa * baoa - radius * radius * baba;
    let h = b * b - a * c;
    if a.abs() > 1e-8 && h >= 0.0 {
        let t = (-b - h.sqrt()) / a;
        let y = baoa + t * bard;
        if t >= 0.0 && y > 0.0 && y < baba {
            return Some(t);
        }
    }
    [
        ray_sphere(origin, direction, from, radius),
        ray_sphere(origin, direction, to, radius),
    ]
    .into_iter()
    .flatten()
    .min_by(f32::total_cmp)
}
