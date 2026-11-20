use std::{
    collections::HashSet,
    io::{self, Stdout},
    time::Duration,
};

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseButton, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::{graph::lane_rgb, state::AppState};

const GOLD: Color = Color::Rgb(219, 193, 134);
const MINT: Color = Color::Rgb(103, 232, 170);
const MUTED: Color = Color::Rgb(137, 157, 164);
const INK: Color = Color::Rgb(12, 22, 27);

type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
    }
}

pub fn run(state: AppState) -> Result<()> {
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal: TuiTerminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    let mut app = TreeTui {
        state,
        top: 0,
        pan: 0,
        detail_scroll: 0,
        search_mode: false,
        tree_area: Rect::default(),
        detail_area: Rect::default(),
        follow_selection: true,
        initialized: false,
        path: HashSet::new(),
        path_key: None,
    };
    loop {
        let selected = app.state.selected.clone();
        let before = selected
            .as_deref()
            .and_then(|id| app.state.graph.index.get(id))
            .map(|&index| {
                (
                    app.state.graph.nodes[index].display_row,
                    app.state.graph.rails.lanes[index] as i32,
                )
            });
        let revision = app.state.revision;
        app.state.poll();
        if app.state.selected != selected {
            app.follow_selection = true;
            app.detail_scroll = 0;
        }
        if app.state.revision != revision
            && let (Some((row, lane)), Some(&index)) = (
                before,
                selected
                    .as_deref()
                    .and_then(|id| app.state.graph.index.get(id)),
            )
        {
            app.top = app.top.saturating_add_signed(
                app.state.graph.nodes[index].display_row as isize - row as isize,
            );
            app.pan = (app.pan + (app.state.graph.rails.lanes[index] as i32 - lane) * 4).max(0);
        }
        if app.state.revision != revision || app.state.selected != app.path_key {
            app.path = app
                .state
                .selected
                .as_deref()
                .and_then(|id| app.state.graph.index.get(id))
                .map(|&index| app.state.graph.rails.ancestry(index))
                .unwrap_or_default();
            app.path_key.clone_from(&app.state.selected);
        }
        if !app.initialized && app.state.snapshot.is_some() {
            app.initialized = true;
            app.follow_selection = true;
        }
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(100))? && app.handle(event::read()?) {
            break;
        }
    }
    terminal.show_cursor()?;
    Ok(())
}

struct TreeTui {
    state: AppState,
    top: usize,
    pan: i32,
    detail_scroll: u16,
    search_mode: bool,
    tree_area: Rect,
    detail_area: Rect,
    follow_selection: bool,
    initialized: bool,
    path: HashSet<usize>,
    path_key: Option<String>,
}

impl TreeTui {
    fn select_row(&mut self, delta: isize) {
        let row = self
            .state
            .selected
            .as_deref()
            .and_then(|id| self.state.graph.index.get(id))
            .copied()
            .unwrap_or(0);
        let row = row
            .saturating_add_signed(delta)
            .min(self.state.graph.nodes.len().saturating_sub(1));
        if let Some(node) = self.state.graph.nodes.get(row) {
            self.state.select(node.id.clone());
            self.follow_selection = true;
            self.detail_scroll = 0;
        }
    }

    fn graph_width(&self) -> i32 {
        let span = self.state.graph.rails.width as i32 * 4 + 2;
        span.min((i32::from(self.tree_area.width) / 2).max(4))
    }

    fn keep_selection_visible(&mut self) {
        if !self.follow_selection {
            return;
        }
        self.follow_selection = false;
        if let Some(node) = self
            .state
            .selected
            .as_deref()
            .and_then(|id| self.state.graph.node(id))
        {
            let y = node.display_row;
            let height = usize::from(self.tree_area.height).max(1);
            if y < self.top || y >= self.top + height.saturating_sub(1) {
                self.top = y.saturating_sub(height / 3);
            }
            let index = self.state.graph.index[&node.id];
            let x = self.state.graph.rails.lanes[index] as i32 * 4 + 1;
            let width = self.graph_width();
            if x - self.pan < 1 || x - self.pan >= width - 1 {
                self.pan = (x - width / 2).max(0);
            }
        }
    }

    fn handle(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    return true;
                }
                if self.search_mode {
                    match key.code {
                        KeyCode::Esc => self.search_mode = false,
                        KeyCode::Enter => {
                            self.search_mode = false;
                            self.state.step_match();
                            self.follow_selection = true;
                            self.detail_scroll = 0;
                        }
                        KeyCode::Backspace => {
                            self.state.query.pop();
                            self.state.update_search();
                        }
                        KeyCode::Char(character) => {
                            self.state.query.push(character);
                            self.state.update_search();
                        }
                        _ => {}
                    }
                    return false;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return true,
                    KeyCode::Enter => self.state.request_details(),
                    KeyCode::Char('j') | KeyCode::Down => self.select_row(1),
                    KeyCode::Char('k') | KeyCode::Up => self.select_row(-1),
                    KeyCode::Home => self.select_row(-(self.state.graph.nodes.len() as isize)),
                    KeyCode::End => self.select_row(self.state.graph.nodes.len() as isize),
                    KeyCode::PageDown => self.detail_scroll = self.detail_scroll.saturating_add(12),
                    KeyCode::PageUp => self.detail_scroll = self.detail_scroll.saturating_sub(12),
                    KeyCode::Left => self.pan = (self.pan - 4).max(0),
                    KeyCode::Right => self.pan += 4,
                    KeyCode::Char('/') => self.search_mode = true,
                    KeyCode::Char('n') => {
                        self.state.step_match();
                        self.follow_selection = true;
                        self.detail_scroll = 0;
                    }
                    KeyCode::Char('h') => {
                        self.state.select_head();
                        self.follow_selection = true;
                        self.detail_scroll = 0;
                    }
                    KeyCode::Char('r') => self.state.worker.refresh(),
                    _ => {}
                }
            }
            Event::Mouse(mouse) => {
                let position = ratatui::layout::Position::new(mouse.column, mouse.row);
                if self.tree_area.contains(position) {
                    match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            let y = self.top + usize::from(mouse.row - self.tree_area.y);
                            let row = self
                                .state
                                .graph
                                .nodes
                                .partition_point(|node| node.display_row <= y)
                                .saturating_sub(1);
                            if let Some(node) = self.state.graph.nodes.get(row) {
                                self.state.select(node.id.clone());
                                self.detail_scroll = 0;
                            }
                        }
                        MouseEventKind::ScrollDown => {
                            self.top = (self.top + 4).min(
                                self.state
                                    .graph
                                    .nodes
                                    .last()
                                    .map_or(0, |node| node.display_row),
                            )
                        }
                        MouseEventKind::ScrollUp => self.top = self.top.saturating_sub(4),
                        _ => {}
                    }
                } else if self.detail_area.contains(position) {
                    match mouse.kind {
                        MouseEventKind::ScrollDown => {
                            self.detail_scroll = self.detail_scroll.saturating_add(3)
                        }
                        MouseEventKind::ScrollUp => {
                            self.detail_scroll = self.detail_scroll.saturating_sub(3)
                        }
                        _ => {}
                    }
                }
            }
            Event::Resize(_, _) => self.follow_selection = true,
            _ => {}
        }
        false
    }

    fn draw(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        frame.render_widget(
            Block::default().style(Style::default().bg(INK).fg(Color::White)),
            area,
        );
        let sections = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .split(area);
        let title = if self.search_mode {
            format!(
                " / {}_  (Enter: next match, Esc: close)",
                safe(&self.state.query)
            )
        } else {
            format!(
                " GIT TREE / {}    {}",
                safe(&self.state.worker.location.title),
                if self.state.query.is_empty() {
                    self.state.worker.location.path.display().to_string()
                } else {
                    format!(
                        "Search: {} [{} matches]",
                        safe(&self.state.query),
                        self.state.matches.len()
                    )
                }
            )
        };
        frame.render_widget(
            Paragraph::new(title)
                .style(Style::default().fg(GOLD).add_modifier(Modifier::BOLD))
                .block(Block::default().borders(Borders::BOTTOM)),
            sections[0],
        );
        let columns = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)])
            .split(sections[1]);
        let tree_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(50, 75, 73)))
            .title(" History / fixed rows ");
        self.tree_area = tree_block.inner(columns[0]);
        frame.render_widget(tree_block, columns[0]);
        let detail_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(50, 75, 73)))
            .title(" Commit inspector / PgUp PgDn ");
        self.detail_area = detail_block.inner(columns[1]);
        frame.render_widget(detail_block, columns[1]);
        self.keep_selection_visible();
        self.draw_tree(frame);
        let detail = safe(&self.state.detail_text());
        let lines: Vec<Line<'_>> = detail
            .lines()
            .map(|line| {
                let color = if line.starts_with('+') {
                    MINT
                } else if line.starts_with('-') {
                    Color::LightRed
                } else if line.starts_with("@@") {
                    Color::LightBlue
                } else {
                    Color::Rgb(190, 205, 205)
                };
                Line::styled(line, Style::default().fg(color))
            })
            .collect();
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((self.detail_scroll, 0)),
            self.detail_area,
        );
        let mut status = self.state.status();
        if let Some(error) = &self.state.watch_error {
            status.push_str(&format!(" | {error}"));
        }
        if let Some(warning) = self
            .state
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.warnings.first())
        {
            status.push_str(&format!(" | {warning}"));
        }
        frame.render_widget(Paragraph::new(vec![
            Line::styled(safe(&status), Style::default().fg(MUTED)),
            Line::styled("Click/Enter: load changes | j/k: select | scroll: history/details | left/right: pan | /: search | n: next | h: HEAD | r: refresh | q: quit", Style::default().fg(GOLD))
        ]), sections[2]);
    }

    fn draw_tree(&self, frame: &mut Frame<'_>) {
        let area = self.tree_area;
        let Some(snapshot) = &self.state.snapshot else {
            frame.render_widget(
                Paragraph::new("Reading local commits and references...")
                    .style(Style::default().fg(MUTED)),
                area,
            );
            return;
        };
        if snapshot.commits.is_empty() {
            frame.render_widget(
                Paragraph::new("No reachable commits yet.").style(Style::default().fg(GOLD)),
                area,
            );
            return;
        }
        let rails = &self.state.graph.rails;
        let width = self.graph_width();
        let x = |lane: usize| lane as i32 * 4 + 1 - self.pan;
        let bottom = self.top + usize::from(area.height);
        for edge in &rails.edges {
            let y0 = edge.child * 2;
            let y1 = edge.parent * 2;
            if y0 >= bottom {
                break;
            }
            if y1 < self.top {
                continue;
            }
            let [r, g, b] = lane_rgb(edge.route_lane as i32);
            let active = self.path.contains(&edge.child) && self.path.contains(&edge.parent);
            let color = if active {
                Color::Rgb(r, g, b)
            } else {
                Color::Rgb(r / 3, g / 3, b / 3)
            };
            let route = x(edge.route_lane);
            let start = x(edge.from_lane);
            let end = x(edge.to_lane);
            for y in (y0 + 1).max(self.top)..y1.min(bottom) {
                let symbol = if edge.merge {
                    '┊'
                } else if rails.trunk.contains(&edge.child) && rails.trunk.contains(&edge.parent) {
                    '┃'
                } else {
                    '│'
                };
                cell(frame, area, width, route, y - self.top, symbol, color);
            }
            for (left, right, y) in [(start, route, y0 + 1), (route, end, y1.saturating_sub(1))] {
                if left == right || y < self.top || y >= bottom {
                    continue;
                }
                for column in left.min(right).max(0)..=left.max(right).min(width - 1) {
                    cell(
                        frame,
                        area,
                        width,
                        column,
                        y - self.top,
                        if edge.merge { '┄' } else { '─' },
                        color,
                    );
                }
            }
        }
        let first = self.top.div_ceil(2);
        for row in first..snapshot.commits.len() {
            let y = row * 2 - self.top;
            if y >= usize::from(area.height) {
                break;
            }
            let commit = &snapshot.commits[row];
            let selected = self.state.selected.as_ref() == Some(&commit.id);
            let matched = self.state.match_set.contains(&commit.id);
            let [r, g, b] = lane_rgb(rails.lanes[row] as i32);
            let color = if selected {
                GOLD
            } else if self.path.contains(&row) {
                Color::Rgb(r, g, b)
            } else {
                MUTED
            };
            let column = x(rails.lanes[row]);
            if column >= 0
                && column < width
                && column < i32::from(area.width)
                && let Some(cell) = frame
                    .buffer_mut()
                    .cell_mut((area.x + column as u16, area.y + y as u16))
            {
                cell.set_symbol(if self.state.graph.nodes[row].boundary {
                    "◌"
                } else if commit.parents.len() > 1 {
                    "◆"
                } else {
                    "●"
                })
                .set_fg(color);
            }
            let label_x = (width + 1).min(i32::from(area.width)) as u16;
            let label_area = Rect::new(
                area.x + label_x,
                area.y + y as u16,
                area.width.saturating_sub(label_x),
                1,
            );
            let columns =
                Layout::horizontal([Constraint::Length(10), Constraint::Min(8)]).split(label_area);
            let label_style = Style::default()
                .fg(if selected {
                    GOLD
                } else if matched {
                    MINT
                } else {
                    Color::Rgb(187, 202, 205)
                })
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                });
            frame.render_widget(
                Paragraph::new(format!(
                    "{}{}",
                    if selected { "> " } else { "  " },
                    commit.short_id()
                ))
                .style(label_style),
                columns[0],
            );
            let mut labels = snapshot.labels(&commit.id);
            if snapshot.head.as_ref() == Some(&commit.id) {
                labels.insert(0, "HEAD");
            }
            frame.render_widget(
                Paragraph::new(safe(&format!(
                    "{}{}",
                    if labels.is_empty() {
                        String::new()
                    } else {
                        format!("[{}] ", labels.join(","))
                    },
                    commit.subject
                )))
                .style(label_style),
                columns[1],
            );
        }
    }
}

fn cell(
    frame: &mut Frame<'_>,
    area: Rect,
    width: i32,
    x: i32,
    y: usize,
    character: char,
    color: Color,
) {
    if x < 0 || x >= width || x >= i32::from(area.width) || y >= usize::from(area.height) {
        return;
    }
    if let Some(cell) = frame
        .buffer_mut()
        .cell_mut((area.x + x as u16, area.y + y as u16))
    {
        let old = cell.symbol();
        let symbol =
            if !matches!(character, '●' | '◆' | '◌') && old != " " && old != character.to_string()
            {
                "┼".to_owned()
            } else {
                character.to_string()
            };
        cell.set_symbol(&symbol).set_fg(color);
    }
}

fn safe(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect::<String>()
        .replace('\t', "    ")
}
