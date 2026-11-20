use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Result;
use eframe::egui::{self, Align2, Color32, FontId, PointerButton, Pos2, Rect, Sense, Stroke, Vec2};
use glam::Vec3;

use crate::{
    View,
    history::HistoryTable,
    renderer::{RenderFrame, Renderer},
    scene::{Camera, Scene, ray_sphere},
    state::AppState,
};

const GOLD: Color32 = Color32::from_rgb(219, 193, 134);
const MINT: Color32 = Color32::from_rgb(103, 232, 170);
const MUTED: Color32 = Color32::from_rgb(137, 157, 164);
const INK: Color32 = Color32::from_rgb(12, 22, 27);

pub fn run(state: AppState, view: View) -> Result<()> {
    let title = format!("Git Tree - {}", state.worker.location.title);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([900.0, 580.0]),
        renderer: eframe::Renderer::Glow,
        depth_buffer: 24,
        multisampling: 4,
        ..Default::default()
    };
    eframe::run_native(
        "Git Tree",
        options,
        Box::new(move |context| {
            configure(&context.egui_ctx);
            Ok(Box::new(WorldTree {
                state,
                view,
                history: HistoryTable::default(),
                renderer: None,
                graphics_error: None,
                scene_revision: None,
                scene_index: std::collections::HashMap::new(),
                scene: Arc::new(Scene::default()),
                camera: Camera::default(),
                initialized: false,
                fit_pending: true,
                focus_pending: false,
                show_refs: true,
                show_details: view == View::Rails,
                highlight_path: false,
                focus_ref: None,
                focus: None,
                matches: Arc::new(HashSet::new()),
                render_revision: 0,
                rendered_query: String::new(),
            }))
        }),
    )
    .map_err(|error| {
        anyhow::anyhow!("Cannot open GUI: {error}. Use --tui for the terminal interface.")
    })
}

fn configure(context: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(16, 28, 33);
    visuals.window_fill = INK;
    visuals.extreme_bg_color = Color32::from_rgb(9, 18, 23);
    visuals.selection.bg_fill = Color32::from_rgb(40, 89, 76);
    visuals.selection.stroke = Stroke::new(1.0_f32, MINT);
    context.set_visuals(visuals);
    let mut style = (*context.style()).clone();
    style.spacing.item_spacing = Vec2::new(10.0, 8.0);
    style.spacing.button_padding = Vec2::new(9.0, 6.0);
    context.set_style(style);
    for path in [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.otf",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("cjk".to_owned(), egui::FontData::from_owned(bytes).into());
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .push("cjk".to_owned());
            }
            context.set_fonts(fonts);
            break;
        }
    }
}

struct WorldTree {
    state: AppState,
    view: View,
    history: HistoryTable,
    renderer: Option<Arc<Mutex<Renderer>>>,
    graphics_error: Option<String>,
    scene_revision: Option<usize>,
    scene_index: std::collections::HashMap<String, usize>,
    scene: Arc<Scene>,
    camera: Camera,
    initialized: bool,
    fit_pending: bool,
    focus_pending: bool,
    show_refs: bool,
    show_details: bool,
    highlight_path: bool,
    focus_ref: Option<String>,
    focus: Option<Arc<HashSet<usize>>>,
    matches: Arc<HashSet<usize>>,
    render_revision: u64,
    rendered_query: String,
}

struct ProjectedNode {
    index: usize,
    point: Pos2,
    depth: f32,
    radius: f32,
    pixel_radius: f32,
}

impl WorldTree {
    fn sync_style(&mut self) {
        if self.focus_ref.as_ref().is_some_and(|name| {
            self.state.snapshot.as_ref().is_none_or(|snapshot| {
                !snapshot
                    .refs
                    .iter()
                    .any(|reference| &reference.full_name == name)
            })
        }) {
            self.focus_ref = None;
        }
        self.focus = self
            .highlight_path
            .then(|| {
                let snapshot = self.state.snapshot.as_ref()?;
                let id = self
                    .focus_ref
                    .as_ref()
                    .and_then(|name| {
                        snapshot
                            .refs
                            .iter()
                            .find(|reference| &reference.full_name == name)
                            .map(|reference| reference.target.as_str())
                    })
                    .or(self.state.selected.as_deref())?;
                let &index = snapshot.index.get(id)?;
                Some(Arc::new(self.state.graph.rails.ancestry(index)))
            })
            .flatten();
        self.matches = Arc::new(
            self.state
                .matches
                .iter()
                .filter_map(|id| self.state.graph.index.get(id).copied())
                .collect(),
        );
        self.rendered_query.clone_from(&self.state.query);
        self.render_revision = self.render_revision.wrapping_add(1);
    }

    fn focus_selected(&mut self) {
        if let Some(&index) = self
            .state
            .selected
            .as_deref()
            .and_then(|id| self.state.graph.index.get(id))
        {
            let node = &self.scene.nodes[index];
            self.camera.target = node.position;
            self.camera.distance = (node.radius * 65.0).clamp(0.003, 12.0);
        }
    }

    fn refs_panel(&mut self, context: &egui::Context) {
        if !self.show_refs {
            return;
        }
        let mut clicked = None;
        let mut reset_focus = false;
        egui::SidePanel::left("references").default_width(205.0).min_width(160.0).resizable(true).show(context, |ui| {
            ui.add_space(10.0);
            ui.heading(&self.state.worker.location.title);
            ui.label(egui::RichText::new(self.state.worker.location.path.display().to_string()).color(MUTED).size(10.0));
            ui.add_space(8.0);
            if self.highlight_path && ui.button("Show all paths").clicked() { reset_focus = true; }
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                if !self.state.query.trim().is_empty() {
                    ui.label(egui::RichText::new(format!("SEARCH / {}", self.state.matches.len())).color(GOLD).strong());
                    for id in self.state.matches.iter().take(100) {
                        if let Some(commit) = self.state.snapshot.as_ref().and_then(|snapshot| snapshot.commit(id))
                            && ui.selectable_label(self.state.selected.as_ref() == Some(id), format!("{}  {}", commit.short_id(), commit.subject)).clicked()
                        { clicked = Some((id.clone(), None)); }
                    }
                    ui.separator();
                }
                ui.label(egui::RichText::new("BRANCHES & TAGS").color(GOLD).size(11.0).strong());
                if let Some(snapshot) = &self.state.snapshot {
                    for reference in &snapshot.refs {
                        let enabled = snapshot.index.contains_key(&reference.target);
                        let response = ui.add_enabled(enabled, egui::Button::selectable(
                            self.focus_ref.as_ref() == Some(&reference.full_name),
                            egui::RichText::new(&reference.name).color(if reference.name == snapshot.trunk_name { MINT } else { Color32::from_rgb(185, 200, 202) })
                        ));
                        if response.clicked() { clicked = Some((reference.target.clone(), Some(reference.full_name.clone()))); }
                        response.on_hover_text(&reference.full_name);
                    }
                    if snapshot.refs.is_empty() { ui.label("No references yet."); }
                }
                ui.add_space(22.0);
                ui.label(egui::RichText::new("Read the history.\nFollow the changes.").color(MUTED).size(12.0));
                ui.add_space(12.0);
                ui.label(egui::RichText::new(if self.view == View::Rails {
                    "Click a row to inspect\nUp/Down: select\nH: HEAD / Enter: locate\nR: refresh / Esc: clear path"
                } else { "Drag: rotate / right drag: pan\nWheel: zoom / double click: focus\nF: fit / H: HEAD / R: refresh" }).color(MUTED).size(11.0));
            });
        });
        if reset_focus {
            self.highlight_path = false;
            self.focus_ref = None;
            self.sync_style();
        }
        if let Some((id, reference)) = clicked {
            self.state.select(id);
            self.highlight_path = true;
            self.focus_ref = reference;
            self.sync_style();
            self.focus_pending = true;
            self.show_details = true;
        }
    }

    fn details_panel(&mut self, context: &egui::Context) {
        if !self.show_details {
            return;
        }
        let mut request_changes = false;
        egui::SidePanel::right("details")
            .default_width(350.0)
            .width_range(250.0..=760.0)
            .resizable(true)
            .show(context, |ui| {
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("COMMIT / INSPECTOR")
                        .color(GOLD)
                        .size(11.0)
                        .strong(),
                );
                let Some(commit) = self.state.selected_commit() else {
                    ui.label("Select a commit row to inspect its details.");
                    return;
                };
                egui::ScrollArea::vertical()
                    .id_salt(("commit-details", &commit.id))
                    .show(ui, |ui| {
                        ui.heading(&commit.subject);
                        ui.horizontal(|ui| {
                            ui.monospace(commit.short_id());
                            if ui.small_button("Copy ID").clicked() {
                                ui.ctx().copy_text(commit.id.clone());
                            }
                        });
                        ui.label(format!("{} <{}>", commit.author, commit.email));
                        ui.label(egui::RichText::new(commit.date()).color(MUTED));
                        ui.collapsing("Identity & parents", |ui| {
                            ui.monospace(&commit.id);
                            if commit.parents.is_empty() {
                                ui.label("Root commit");
                            }
                            for parent in &commit.parents {
                                ui.monospace(parent);
                            }
                        });
                        ui.separator();
                        ui.label(&commit.message);
                        ui.separator();
                        if let Some(error) = &self.state.detail_error {
                            ui.colored_label(Color32::LIGHT_RED, error);
                            if ui.button("Retry file changes").clicked() {
                                request_changes = true;
                            }
                        } else if let Some(details) = &self.state.details {
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} CHANGED FILES",
                                    details.files.len()
                                ))
                                .color(GOLD)
                                .strong(),
                            );
                            ui.label(
                                egui::RichText::new(
                                    "Compared with first parent. Renames appear as add/delete.",
                                )
                                .size(10.0)
                                .color(MUTED),
                            );
                            for file in &details.files {
                                ui.horizontal_wrapped(|ui| {
                                    let color = match file.status {
                                        "added" => MINT,
                                        "deleted" => Color32::LIGHT_RED,
                                        _ => GOLD,
                                    };
                                    ui.label(
                                        egui::RichText::new(file.status)
                                            .color(color)
                                            .monospace()
                                            .size(11.0),
                                    );
                                    ui.label(&file.path);
                                });
                            }
                            ui.separator();
                            egui::ScrollArea::both()
                                .id_salt(("diff-scroll", &commit.id))
                                .max_height(480.0)
                                .show_rows(ui, 14.0, details.diff_lines.len(), |ui, rows| {
                                    for line in &details.diff_lines[rows] {
                                        let color = if line.starts_with('+') {
                                            MINT
                                        } else if line.starts_with('-') {
                                            Color32::from_rgb(244, 151, 157)
                                        } else if line.starts_with("@@") {
                                            Color32::LIGHT_BLUE
                                        } else {
                                            Color32::from_rgb(177, 192, 195)
                                        };
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(line)
                                                    .monospace()
                                                    .size(11.0)
                                                    .color(color),
                                            )
                                            .extend(),
                                        );
                                    }
                                });
                        } else if self.state.details_requested {
                            ui.spinner();
                            ui.label("Reading file changes...");
                        } else {
                            ui.label("File changes are loaded only on request.");
                            if ui.button("Load file changes").clicked() {
                                request_changes = true;
                            }
                        }
                    });
            });
        if request_changes {
            self.state.request_details();
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = response.rect;
        if !rect.is_positive() {
            return;
        }
        painter.rect_filled(rect, 0.0, INK);
        draw_sky(&painter, rect);
        if self.fit_pending && !self.scene.nodes.is_empty() {
            self.camera.fit(&self.scene, rect);
            self.fit_pending = false;
            self.focus_pending = false;
        }
        if self.focus_pending {
            self.focus_selected();
            self.focus_pending = false;
        }
        let delta = ui.input(|input| input.pointer.delta());
        if response.dragged_by(PointerButton::Primary) {
            self.camera.orbit(delta);
        }
        if response.dragged_by(PointerButton::Secondary)
            || response.dragged_by(PointerButton::Middle)
        {
            self.camera.pan(delta, rect);
        }
        if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            self.camera.distance = (self.camera.distance * (-scroll * 0.0025).exp())
                .clamp(0.0002, self.scene.radius() * 200.0);
        }
        let Some(snapshot) = &self.state.snapshot else {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Reading local commits and references...",
                FontId::proportional(22.0),
                GOLD,
            );
            return;
        };
        if self.scene.nodes.is_empty() {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "No reachable commits yet.",
                FontId::proportional(22.0),
                GOLD,
            );
            return;
        }
        let matrix = self.camera.matrix(rect, self.scene.radius());
        let selected = self
            .state
            .selected
            .as_deref()
            .and_then(|id| self.state.graph.index.get(id))
            .copied();
        let head = snapshot
            .head
            .as_deref()
            .and_then(|id| self.state.graph.index.get(id))
            .copied();
        let mut projected = Vec::new();
        for (index, node) in self.scene.nodes.iter().enumerate() {
            let Some((point, depth)) = Camera::project(matrix, rect, node.position) else {
                continue;
            };
            let unit = self.camera.units_per_point(rect) * depth / self.camera.distance;
            let important = node.important
                || selected == Some(index)
                || head == Some(index)
                || self.matches.contains(&index);
            let radius = node.radius.max(if important { unit * 2.2 } else { 0.0 })
                * if selected == Some(index) { 1.4 } else { 1.0 };
            if !important && radius / unit < 1.5 {
                continue;
            }
            projected.push(ProjectedNode {
                index,
                point,
                depth,
                radius,
                pixel_radius: radius / unit,
            });
        }
        let priority = |index| {
            if selected == Some(index) {
                0
            } else if head == Some(index) || self.matches.contains(&index) {
                1
            } else if self.scene.nodes[index].important {
                2
            } else {
                3
            }
        };
        projected.sort_by(|a, b| {
            priority(a.index)
                .cmp(&priority(b.index))
                .then(a.depth.total_cmp(&b.depth))
        });
        let mut occupied = HashSet::new();
        projected.retain(|node| {
            let cell = ((node.point.x / 5.0) as i32, (node.point.y / 5.0) as i32);
            let free = !occupied.contains(&cell);
            let keep = priority(node.index) < 3 || free;
            if keep {
                for x in -1..=1 {
                    for y in -1..=1 {
                        occupied.insert((cell.0 + x, cell.1 + y));
                    }
                }
            }
            keep
        });
        let pixel_scale = self.camera.units_per_point(rect) / self.camera.distance;
        let hovered = response.hover_pos().and_then(|pointer| {
            let (origin, direction) = self.camera.ray(matrix, rect, pointer);
            projected
                .iter()
                .filter_map(|node| {
                    if pointer.distance(node.point) > node.pixel_radius + 2.0 {
                        return None;
                    }
                    ray_sphere(
                        origin,
                        direction,
                        self.scene.nodes[node.index].position,
                        node.radius,
                    )
                    .map(|distance| (node.index, distance))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .filter(|(_, distance)| {
                    !self
                        .scene
                        .occluded(origin, direction, *distance, pixel_scale)
                })
                .map(|(index, _)| index)
        });
        let Some(renderer) = self.renderer.as_ref().map(Arc::clone) else {
            return;
        };
        let scene = Arc::clone(&self.scene);
        let frame = RenderFrame {
            matrix,
            eye: self.camera.eye(),
            pixel_scale,
            selected,
            hovered,
            visible: projected
                .iter()
                .map(|node| (node.index, node.radius))
                .collect(),
            focus: self.focus.clone(),
            matches: Arc::clone(&self.matches),
            revision: self.render_revision,
        };
        painter.add(egui::PaintCallback {
            rect,
            callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |_info, painter| {
                if let Ok(mut renderer) = renderer.lock() {
                    renderer.paint(painter.gl(), &scene, &frame);
                }
            })),
        });
        let mut labels = Vec::new();
        let mut label_nodes: Vec<_> = projected.iter().collect();
        label_nodes.sort_by_key(|node| {
            if Some(node.index) == hovered {
                0
            } else if Some(node.index) == selected {
                1
            } else {
                2
            }
        });
        for node in label_nodes {
            let commit = &snapshot.commits[node.index];
            let active = Some(node.index) == hovered || Some(node.index) == selected;
            let refs = snapshot.labels(&commit.id);
            if !active && (refs.is_empty() || labels.len() >= 6) {
                continue;
            }
            let (origin, direction) = self.camera.ray(matrix, rect, node.point);
            let distance = origin.distance(self.scene.nodes[node.index].position) - node.radius;
            if self
                .scene
                .occluded(origin, direction, distance, pixel_scale)
            {
                continue;
            }
            if projected.iter().any(|other| {
                other.depth < node.depth - node.radius
                    && other.point.distance(node.point) < other.pixel_radius
            }) {
                continue;
            }
            let text = if active {
                format!(
                    "{}  {}",
                    commit.short_id(),
                    commit.subject.chars().take(42).collect::<String>()
                )
            } else {
                refs.into_iter().take(2).collect::<Vec<_>>().join(" / ")
            };
            let galley = painter.layout_no_wrap(
                text,
                FontId::proportional(if active { 12.0 } else { 11.0 }),
                if active { GOLD } else { MUTED },
            );
            let at = node.point + Vec2::new(node.pixel_radius + 7.0, -galley.size().y * 0.5);
            let label =
                Rect::from_min_size(at - Vec2::splat(4.0), galley.size() + Vec2::splat(8.0));
            if !rect.shrink(10.0).contains_rect(label)
                || labels
                    .iter()
                    .any(|other: &Rect| other.expand(7.0).intersects(label))
            {
                continue;
            }
            painter.rect_filled(label, 3.0, Color32::from_rgba_unmultiplied(12, 22, 27, 230));
            painter.galley(at, galley, MUTED);
            labels.push(label);
        }
        if let Some(index) = hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            let commit = &snapshot.commits[index];
            response.clone().on_hover_ui(|ui| {
                ui.strong(&commit.subject);
                ui.monospace(commit.short_id());
                ui.label(format!("{} / {}", commit.author, commit.date()));
                ui.label("Click: load file changes / double click: focus");
            });
            if response.clicked() || response.double_clicked() {
                self.state.select(commit.id.clone());
                self.focus_ref = None;
                self.highlight_path = true;
                self.show_details = true;
            }
            if response.double_clicked() {
                self.focus_pending = true;
            }
        }
        if response.clicked() || response.double_clicked() {
            self.sync_style();
        }
        painter.text(
            rect.left_top() + Vec2::new(24.0, 22.0),
            Align2::LEFT_TOP,
            "GIT / 3D",
            FontId::proportional(12.0),
            GOLD,
        );
        painter.text(
            rect.left_bottom() + Vec2::new(24.0, -22.0),
            Align2::LEFT_BOTTOM,
            format!(
                "{} / {} commits  |  drag: orbit  |  right drag: pan  |  scroll: zoom",
                projected.len(),
                self.scene.nodes.len()
            ),
            FontId::monospace(10.0),
            MUTED,
        );
        draw_orientation(&painter, rect, &self.camera);
    }
}

impl eframe::App for WorldTree {
    fn update(&mut self, context: &egui::Context, frame: &mut eframe::Frame) {
        let selected_before = self.state.selected.clone();
        let previous_position = selected_before
            .as_deref()
            .and_then(|id| self.scene_index.get(id))
            .and_then(|&index| self.scene.nodes.get(index))
            .map(|node| node.position);
        let revision = self.state.revision;
        self.state.poll();
        if self.state.revision != revision {
            if let Some(snapshot) = &self.state.snapshot {
                self.history.preserve_anchor(snapshot);
            }
            self.sync_style();
        }
        if !context.wants_keyboard_input() {
            context.input(|input| {
                if input.key_pressed(egui::Key::R) {
                    self.state.worker.refresh();
                }
                if input.key_pressed(egui::Key::H) {
                    self.state.select_head();
                    self.show_details = true;
                    self.focus_ref = None;
                    self.focus_pending = true;
                    self.sync_style();
                }
                if input.key_pressed(egui::Key::Enter) {
                    self.state.request_details();
                    self.show_details = true;
                    self.focus_pending = true;
                }
                if self.view == View::Rails {
                    let delta = if input.key_pressed(egui::Key::ArrowDown) {
                        1
                    } else if input.key_pressed(egui::Key::ArrowUp) {
                        -1
                    } else {
                        0
                    };
                    if delta != 0 {
                        let index = self
                            .state
                            .selected
                            .as_deref()
                            .and_then(|id| self.state.graph.index.get(id))
                            .copied()
                            .unwrap_or(0);
                        let index = index
                            .saturating_add_signed(delta)
                            .min(self.state.graph.nodes.len().saturating_sub(1));
                        if let Some(node) = self.state.graph.nodes.get(index) {
                            self.state.select(node.id.clone());
                            self.focus_ref = None;
                            self.highlight_path = true;
                            self.focus_pending = true;
                        }
                    }
                } else if input.key_pressed(egui::Key::F) {
                    self.fit_pending = true;
                }
                if input.key_pressed(egui::Key::Escape) {
                    self.focus_ref = None;
                    self.highlight_path = false;
                    self.sync_style();
                }
            });
        }
        let previous_view = self.view;
        egui::TopBottomPanel::top("toolbar").show(context, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new("GIT TREE")
                        .color(GOLD)
                        .strong()
                        .size(17.0),
                );
                ui.selectable_value(&mut self.view, View::Graph3d, "Git 3D");
                ui.selectable_value(&mut self.view, View::Rails, "History 2D");
                ui.separator();
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.state.query)
                        .hint_text("Search commit, author or branch...")
                        .desired_width(205.0),
                );
                if search.changed() {
                    self.state.update_search();
                }
                if ui.button("Next match").clicked()
                    || (search.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter)))
                {
                    self.state.step_match();
                    self.focus_ref = None;
                    self.highlight_path = true;
                    self.focus_pending = true;
                    self.show_details = true;
                    self.sync_style();
                }
                if ui.button("HEAD").clicked() {
                    self.state.select_head();
                    self.show_details = true;
                    self.focus_ref = None;
                    self.focus_pending = true;
                    self.sync_style();
                }
                if self.view == View::Rails {
                    if ui.button("Locate").clicked() {
                        self.focus_pending = true;
                    }
                } else {
                    if ui.button("Fit graph").clicked() {
                        self.fit_pending = true;
                    }
                    if ui.button("Reset view").clicked() {
                        self.camera = Camera::default();
                        self.fit_pending = true;
                    }
                }
                if ui.button("Refresh").clicked() {
                    self.state.worker.refresh();
                }
                if ui.toggle_value(&mut self.highlight_path, "Path").changed() {
                    self.sync_style();
                }
                ui.toggle_value(&mut self.show_refs, "Branches");
                ui.toggle_value(&mut self.show_details, "Inspector");
            });
        });
        if self.view != previous_view {
            self.focus_pending = true;
            self.sync_style();
        }
        if self.state.selected != selected_before || self.rendered_query != self.state.query {
            self.sync_style();
        }
        egui::TopBottomPanel::bottom("status").show(context, |ui| {
            ui.label(
                egui::RichText::new(self.state.status())
                    .color(if self.state.error.is_some() {
                        Color32::LIGHT_RED
                    } else {
                        MUTED
                    })
                    .size(11.0),
            );
            if let Some(error) = &self.state.watch_error {
                ui.colored_label(GOLD, error);
            }
            if let Some(snapshot) = &self.state.snapshot {
                for warning in &snapshot.warnings {
                    ui.colored_label(GOLD, warning);
                }
            }
        });
        self.refs_panel(context);
        if self.view == View::Graph3d {
            if self.renderer.is_none() && self.graphics_error.is_none() {
                match frame
                    .gl()
                    .ok_or_else(|| anyhow::anyhow!("OpenGL context unavailable"))
                    .and_then(|gl| Renderer::new(gl))
                {
                    Ok(renderer) => self.renderer = Some(Arc::new(Mutex::new(renderer))),
                    Err(error) => self.graphics_error = Some(format!("{error:#}")),
                }
            }
            if self.scene_revision != Some(self.state.revision)
                && let Some(snapshot) = &self.state.snapshot
            {
                self.scene = Arc::new(Scene::build(&self.state.graph, snapshot));
                self.scene_revision = Some(self.state.revision);
                self.scene_index.clone_from(&self.state.graph.index);
                if self.initialized {
                    if let (Some(before), Some(after)) = (
                        previous_position,
                        selected_before
                            .as_deref()
                            .and_then(|id| self.state.graph.index.get(id))
                            .and_then(|&index| self.scene.nodes.get(index))
                            .map(|node| node.position),
                    ) {
                        self.camera.target += after - before;
                    }
                } else {
                    self.initialized = true;
                    self.fit_pending = true;
                }
            }
        }
        self.details_panel(context);
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(context, |ui| {
            if self.view == View::Rails {
                if self.focus_pending {
                    if let Some(&index) = self.state.selected.as_deref().and_then(|id|self.state.graph.index.get(id)) {
                        self.history.focus_row(index, ui.available_height());
                    }
                    self.focus_pending = false;
                }
                if self.history.show(ui, &mut self.state, self.focus.as_deref()) {
                    self.focus_ref = None; self.highlight_path = true; self.show_details = true; self.sync_style();
                }
            } else if let Some(error) = &self.graphics_error {
                ui.centered_and_justified(|ui| { ui.colored_label(Color32::LIGHT_RED,format!("Git 3D initialization failed: {error}. Select History 2D to open the 2D view.")); });
            } else { self.canvas(ui); }
        });
        context.request_repaint_after(Duration::from_millis(100));
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let (Some(gl), Some(renderer)) = (gl, &self.renderer)
            && let Ok(mut renderer) = renderer.lock()
        {
            renderer.destroy(gl);
        }
    }
}

fn draw_sky(painter: &egui::Painter, rect: Rect) {
    for index in 0..32_u32 {
        let x = (index.wrapping_mul(7919).wrapping_add(29) % 1000) as f32 / 1000.0 * rect.width();
        let y = (index.wrapping_mul(3571).wrapping_add(103) % 1000) as f32 / 1000.0 * rect.height();
        painter.circle_filled(
            rect.left_top() + Vec2::new(x, y),
            0.6,
            Color32::from_rgb(34, 48, 51),
        );
    }
}

fn draw_orientation(painter: &egui::Painter, rect: Rect, camera: &Camera) {
    let center = rect.right_bottom() - Vec2::new(48.0, 55.0);
    let view = glam::Mat4::look_at_rh(camera.eye(), camera.target, Vec3::Y);
    for (axis, label, color) in [
        (Vec3::X, "X", Color32::from_rgb(183, 116, 120)),
        (Vec3::Y, "UP", MINT),
        (Vec3::Z, "Z", Color32::from_rgb(115, 151, 186)),
    ] {
        let direction = view.transform_vector3(axis);
        let end = center + Vec2::new(direction.x, -direction.y) * 23.0;
        painter.line_segment([center, end], Stroke::new(1.2_f32, color));
        painter.text(
            end,
            Align2::CENTER_BOTTOM,
            label,
            FontId::monospace(9.0),
            color,
        );
    }
}
