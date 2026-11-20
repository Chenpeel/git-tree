use std::collections::HashSet;

use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use crate::{graph::lane_rgb, repository::Snapshot, state::AppState};

pub const ROW_HEIGHT: f32 = 44.0;
const HEADER_HEIGHT: f32 = 30.0;
const LANE_WIDTH: f32 = 18.0;
const TEXT: Color32 = Color32::from_rgb(213, 222, 224);
const MUTED: Color32 = Color32::from_rgb(133, 151, 159);
const GREEN: Color32 = Color32::from_rgb(104, 211, 156);
const GOLD: Color32 = Color32::from_rgb(231, 199, 133);

#[derive(Default)]
pub struct HistoryTable {
    pub offset: Vec2,
    pending_y: Option<f32>,
    anchor: Option<(String, f32)>,
}

impl HistoryTable {
    pub fn focus_row(&mut self, row: usize, height: f32) {
        self.pending_y = Some((row as f32 * ROW_HEIGHT - height.max(120.0) * 0.3).max(0.0));
    }

    pub fn preserve_anchor(&mut self, snapshot: &Snapshot) {
        if let Some((id, fraction)) = &self.anchor
            && let Some(&row) = snapshot.index.get(id)
        {
            self.pending_y = Some(row as f32 * ROW_HEIGHT + fraction);
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        state: &mut AppState,
        focus: Option<&HashSet<usize>>,
    ) -> bool {
        let Some(snapshot) = &state.snapshot else {
            ui.centered_and_justified(|ui| {
                ui.spinner();
                ui.label("Reading local commits and references...");
            });
            return false;
        };
        if snapshot.commits.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("This repository has no reachable commits yet.");
            });
            return false;
        }
        let rails = &state.graph.rails;
        let graph_width = (rails.width as f32 * LANE_WIDTH + 24.0).max(64.0);
        let fixed = graph_width + 72.0 + 106.0 + 130.0;
        let subject_width = (ui.available_width() - fixed - 16.0).max(210.0);
        let widths = [graph_width, 72.0, subject_width, 106.0, 130.0];
        let mut columns = [0.0; 6];
        for (index, width) in widths.iter().enumerate() {
            columns[index + 1] = columns[index] + width;
        }
        let table_width = columns[5];
        let total_height = HEADER_HEIGHT + snapshot.commits.len() as f32 * ROW_HEIGHT;
        let mut scroll = egui::ScrollArea::both()
            .id_salt("commit-history-table")
            .auto_shrink([false, false]);
        if let Some(y) = self.pending_y.take() {
            scroll = scroll.vertical_scroll_offset(y);
        }
        let mut clicked = None;
        let output = scroll.show_viewport(ui, |ui, viewport| {
            ui.set_min_size(Vec2::new(table_width, total_height));
            let origin = ui.min_rect().min;
            let painter = ui.painter().clone();
            let header_top = origin.y + viewport.min.y;
            let content_clip = Rect::from_min_max(
                Pos2::new(ui.clip_rect().left(), (header_top + HEADER_HEIGHT).max(ui.clip_rect().top())),
                ui.clip_rect().max,
            );
            let row_painter = painter.with_clip_rect(content_clip);
            let first = (viewport.min.y / ROW_HEIGHT).floor().max(0.0) as usize;
            let end = ((viewport.max.y / ROW_HEIGHT).ceil() as usize + 1).min(snapshot.commits.len());
            let rail_clip = content_clip.intersect(Rect::from_min_max(
                Pos2::new(origin.x, content_clip.top()), Pos2::new(origin.x + graph_width, content_clip.bottom())));
            let rail_painter = row_painter.with_clip_rect(rail_clip);
            for row in first..end {
                let commit = &snapshot.commits[row];
                let top = origin.y + HEADER_HEIGHT + row as f32 * ROW_HEIGHT;
                let rect = Rect::from_min_size(Pos2::new(origin.x, top), Vec2::new(table_width, ROW_HEIGHT));
                if !rect.intersects(content_clip) { continue; }
                let selected = state.selected.as_ref() == Some(&commit.id);
                let response = ui.interact(rect.intersect(content_clip), ui.id().with(("commit", &commit.id)), Sense::click());
                let fill = if selected { Color32::from_rgb(34, 61, 64) }
                    else if response.hovered() { Color32::from_rgb(28, 43, 51) }
                    else if row % 2 == 0 { Color32::from_rgb(18, 29, 36) }
                    else { Color32::from_rgb(16, 26, 33) };
                row_painter.rect_filled(rect, 0.0, fill);
                if selected { row_painter.rect_filled(Rect::from_min_size(rect.min, Vec2::new(2.0, ROW_HEIGHT)), 0.0, GOLD); }
                let active = focus.is_none_or(|path| path.contains(&row));
                let color = if active || selected { TEXT } else { MUTED.gamma_multiply(0.75) };
                let matched = state.match_set.contains(&commit.id);
                text_cell(&row_painter, Pos2::new(origin.x + columns[1] + 6.0, top + 15.0), widths[1]-8.0,
                    commit.short_id(), 11.0, if selected { GOLD } else { MUTED }, true);
                text_cell(&row_painter, Pos2::new(origin.x + columns[2] + 6.0, top + 5.0), widths[2]-12.0,
                    &commit.subject, 13.0, if matched { GREEN } else { color }, false);
                let mut labels = snapshot.labels(&commit.id);
                if snapshot.head.as_ref() == Some(&commit.id) { labels.insert(0, "HEAD"); }
                if !labels.is_empty() {
                    text_cell(&row_painter, Pos2::new(origin.x + columns[2] + 6.0, top + 24.0), widths[2]-12.0,
                        &labels.join("  /  "), 10.0, if selected { GOLD } else { GREEN }, false);
                } else {
                    text_cell(&row_painter, Pos2::new(origin.x + columns[2] + 6.0, top + 25.0), widths[2]-12.0,
                        if commit.parents.len()>1 { "merge commit" } else { "" }, 10.0, MUTED, false);
                }
                text_cell(&row_painter, Pos2::new(origin.x + columns[3] + 6.0, top + 15.0), widths[3]-12.0,
                    &commit.author, 11.0, color, false);
                let date = commit.date();
                let (day, time) = date.split_once(' ').unwrap_or((&date, ""));
                text_cell(&row_painter, Pos2::new(origin.x + columns[4] + 6.0, top + 6.0), widths[4]-12.0, day, 11.0, color, true);
                text_cell(&row_painter, Pos2::new(origin.x + columns[4] + 6.0, top + 24.0), widths[4]-12.0, time, 10.0, MUTED, true);
                if response.clicked() { clicked = Some(commit.id.clone()); }
                response.on_hover_ui(|ui| {
                    ui.strong(&commit.subject); ui.label(format!("{} / {}",commit.author,commit.date()));
                    ui.small("Click to load file changes on demand. Tracks are layout channels, not permanent branch ownership.");
                });
            }
            for edge in &rails.edges {
                if edge.child >= end { break; }
                if edge.parent < first { continue; }
                let child_y = origin.y + HEADER_HEIGHT + edge.child as f32 * ROW_HEIGHT + ROW_HEIGHT*0.5;
                let parent_y = origin.y + HEADER_HEIGHT + edge.parent as f32 * ROW_HEIGHT + ROW_HEIGHT*0.5;
                let x = |lane: usize| origin.x + 16.0 + lane as f32 * LANE_WIDTH;
                let from = Pos2::new(x(edge.from_lane), child_y);
                let route = x(edge.route_lane);
                let to = Pos2::new(x(edge.to_lane), parent_y);
                let points = [from, Pos2::new(route,child_y + ROW_HEIGHT*0.5),Pos2::new(route,parent_y-ROW_HEIGHT*0.25),to];
                let rgb = lane_rgb(edge.route_lane as i32);
                let active = focus.is_none_or(|path|path.contains(&edge.child)&&path.contains(&edge.parent));
                let color = Color32::from_rgb(rgb[0],rgb[1],rgb[2]).gamma_multiply(if active {0.9} else {0.2});
                let width = if rails.trunk.contains(&edge.child)&&rails.trunk.contains(&edge.parent) {2.4_f32} else {1.6_f32};
                if edge.from_lane == edge.route_lane && edge.route_lane == edge.to_lane {
                    rail_painter.line_segment([from,to],Stroke::new(width,color));
                } else {
                    rail_painter.add(egui::Shape::line(points.to_vec(),Stroke::new(width,color)));
                }
                if edge.merge {
                    rail_painter.circle_filled(from.lerp(points[1],0.65),2.1,color);
                }
            }
            for row in first..end {
                let commit = &snapshot.commits[row];
                let node = &state.graph.nodes[row];
                let position = Pos2::new(origin.x+16.0+rails.lanes[row] as f32*LANE_WIDTH,
                    origin.y+HEADER_HEIGHT+row as f32*ROW_HEIGHT+ROW_HEIGHT*0.5);
                let selected = state.selected.as_ref()==Some(&commit.id);
                let rgb=lane_rgb(rails.lanes[row] as i32);
                let color=if selected {GOLD} else {Color32::from_rgb(rgb[0],rgb[1],rgb[2])};
                rail_painter.circle_filled(position,5.0,Color32::from_rgb(16,26,33));
                if commit.parents.len()>1 {
                    rail_painter.add(egui::Shape::convex_polygon(vec![position+Vec2::new(0.0,-5.0),position+Vec2::new(5.0,0.0),position+Vec2::new(0.0,5.0),position+Vec2::new(-5.0,0.0)],color,Stroke::NONE));
                } else if node.boundary {rail_painter.circle_stroke(position,4.4,Stroke::new(1.6_f32,color));}
                else {rail_painter.circle_filled(position,4.2,color);}
                if selected||snapshot.head.as_ref()==Some(&commit.id) {rail_painter.circle_stroke(position,7.0,Stroke::new(1.1_f32,GOLD));}
            }
            let header=Rect::from_min_size(Pos2::new(origin.x,header_top),Vec2::new(table_width,HEADER_HEIGHT));
            painter.rect_filled(header,0.0,Color32::from_rgb(23,37,45));
            for (index,label) in ["HISTORY","COMMIT","SUBJECT / REFERENCES","AUTHOR","TIME"].into_iter().enumerate(){
                text_cell(&painter,Pos2::new(origin.x+columns[index]+6.0,header_top+8.0),widths[index]-8.0,label,10.0,MUTED,false);
            }
            painter.line_segment([header.left_bottom(),header.right_bottom()],Stroke::new(1.0_f32,Color32::from_rgb(44,61,69)));
        });
        self.offset = output.state.offset;
        let row = (self.offset.y / ROW_HEIGHT).floor() as usize;
        self.anchor = snapshot
            .commits
            .get(row)
            .map(|commit| (commit.id.clone(), self.offset.y - row as f32 * ROW_HEIGHT));
        if let Some(id) = clicked {
            state.select(id);
            return true;
        }
        false
    }
}

fn text_cell(
    painter: &egui::Painter,
    at: Pos2,
    width: f32,
    text: &str,
    size: f32,
    color: Color32,
    mono: bool,
) {
    let font = if mono {
        FontId::monospace(size)
    } else {
        FontId::proportional(size)
    };
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap.max_width = width.max(1.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    painter.galley(at, painter.layout_job(job), color);
}
