use crate::{
    diff::{self, ChangeKind, FileDiff, Kind, Snapshot},
    file_tree::{self, Row as FileRow},
    highlight::{Colors, Highlighter},
    refresh::{Parameters, Refresher},
    setup::{Action as SetupAction, Setup},
    update::{Message, Release, Updater},
    Options,
};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, ModifierKeyCode, MouseButton, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use std::{
    collections::HashSet,
    io::{self, stdout},
    sync::Arc,
    time::{Duration, Instant},
};

const BG: Color = Color::Rgb(19, 23, 31);
const FG: Color = Color::Rgb(211, 220, 231);
const MUTED: Color = Color::Rgb(132, 148, 170);
const ACCENT: Color = Color::Rgb(114, 203, 231);

#[derive(Clone, Copy, PartialEq)]
enum View {
    Current,
    Unified,
    Split,
}
impl View {
    fn next(self) -> Self {
        match self {
            Self::Current => Self::Unified,
            Self::Unified => Self::Split,
            Self::Split => Self::Current,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Current => "Current",
            Self::Unified => "Unified",
            Self::Split => "Split",
        }
    }
}
struct Previous {
    lines: Vec<diff::Line>,
    top: usize,
    x: u16,
}

enum Drag {
    Resize,
    Scroll { grab: u16 },
}

struct Cleanup;
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = execute!(stdout(), DisableMouseCapture, PopKeyboardEnhancementFlags);
        ratatui::restore();
    }
}

struct App {
    setup: Option<Setup>,
    opts: Options,
    files: Vec<Arc<FileDiff>>,
    highlights: Highlighter,
    refresher: Refresher,
    pending_snapshot: Option<Snapshot>,
    manual_refresh: bool,
    updater: Updater,
    update_offer: Option<Release>,
    update_yes: Rect,
    update_no: Rect,
    selected: usize,
    top: usize,
    x: u16,
    view: View,
    cursor: usize,
    previous: Option<Previous>,
    sidebar: bool,
    file_focus: bool,
    file_state: ListState,
    tree: Vec<FileRow>,
    collapsed: HashSet<String>,
    tree_cursor: usize,
    sidebar_width: Option<u16>,
    sidebar_edge: Rect,
    scrollbar: Rect,
    screen_width: u16,
    drag: Option<Drag>,
    picker: Option<String>,
    pick: usize,
    help: bool,
    status: String,
    list_area: Rect,
    body_area: Rect,
    viewport: usize,
    last_click: Option<(usize, Instant)>,
    last_shift: Option<Instant>,
}

impl App {
    fn new(mut opts: Options, snapshot: Snapshot) -> Self {
        let highlights = Highlighter::new(opts.syntax_languages.clone());
        let updater = Updater::new(opts.check_updates);
        let setup = opts.setup.take();
        let status = std::mem::take(&mut opts.startup_notice);
        let mut files: Vec<Arc<FileDiff>> = snapshot.files.into_iter().map(Arc::new).collect();
        if let Some(file) = files.first_mut() {
            Arc::make_mut(file).load_current();
        }
        let tree = file_tree::rows(files.iter().map(|f| f.name.as_str()), &HashSet::new());
        let tree_cursor = tree.iter().position(|r| r.file == Some(0)).unwrap_or(0);
        Self {
            setup,
            opts,
            files,
            highlights,
            refresher: Refresher::new(),
            pending_snapshot: None,
            manual_refresh: false,
            updater,
            update_offer: None,
            update_yes: Rect::default(),
            update_no: Rect::default(),
            selected: 0,
            top: 0,
            x: 0,
            view: View::Current,
            cursor: 0,
            previous: None,
            sidebar: true,
            file_focus: false,
            file_state: ListState::default(),
            tree,
            collapsed: HashSet::new(),
            tree_cursor,
            sidebar_width: None,
            sidebar_edge: Rect::default(),
            scrollbar: Rect::default(),
            screen_width: 0,
            drag: None,
            picker: None,
            pick: 0,
            help: false,
            status,
            list_area: Rect::default(),
            body_area: Rect::default(),
            viewport: 1,
            last_click: None,
            last_shift: None,
        }
    }

    fn select(&mut self, n: usize) {
        self.highlights.selection_changed();
        self.selected = n.min(self.files.len().saturating_sub(1));
        self.top = 0;
        self.x = 0;
        self.reveal_selected();
        self.cursor = 0;
        self.previous = None;
        if let Some(file) = self.files.get_mut(self.selected) {
            Arc::make_mut(file).load_current();
        }
    }

    fn rebuild_tree(&mut self) {
        self.tree = file_tree::rows(self.files.iter().map(|f| f.name.as_str()), &self.collapsed);
        self.tree_cursor = self.tree_cursor.min(self.tree.len().saturating_sub(1));
        self.clamp_tree_scroll();
    }

    fn reveal_selected(&mut self) {
        let mut changed = false;
        if let Some(file) = self.files.get(self.selected) {
            let mut parent = file.name.as_str();
            while let Some((path, _)) = parent.rsplit_once('/') {
                changed |= self.collapsed.remove(path);
                parent = path;
            }
        }
        if changed {
            self.rebuild_tree();
        }
        self.tree_cursor = self
            .tree
            .iter()
            .position(|r| r.file == Some(self.selected))
            .unwrap_or(0);
        self.ensure_tree_cursor();
    }

    fn clamp_tree_scroll(&mut self) {
        let max = self
            .tree
            .len()
            .saturating_sub(usize::from(self.list_area.height));
        *self.file_state.offset_mut() = self.file_state.offset().min(max);
    }

    fn ensure_tree_cursor(&mut self) {
        let height = usize::from(self.list_area.height.max(1));
        let offset = self.file_state.offset();
        *self.file_state.offset_mut() = if self.tree_cursor < offset {
            self.tree_cursor
        } else if self.tree_cursor >= offset + height {
            self.tree_cursor + 1 - height
        } else {
            offset
        };
        self.clamp_tree_scroll();
    }

    fn toggle_directory(&mut self) {
        if let Some(row) = self.tree.get(self.tree_cursor).filter(|r| r.file.is_none()) {
            let path = row.path.clone();
            if !self.collapsed.remove(&path) {
                self.collapsed.insert(path);
            }
            self.last_click = None;
            self.rebuild_tree();
            self.ensure_tree_cursor();
        }
    }

    fn move_tree(&mut self, delta: isize) {
        self.tree_cursor = self
            .tree_cursor
            .saturating_add_signed(delta)
            .min(self.tree.len().saturating_sub(1));
        if let Some(file) = self.tree.get(self.tree_cursor).and_then(|r| r.file) {
            self.select(file);
        }
        self.ensure_tree_cursor();
    }

    fn thumb(&self) -> (u16, u16) {
        let height = usize::from(self.scrollbar.height);
        if height == 0 {
            return (0, 0);
        }
        let count = self.tree.len().max(1);
        let size = (height * height / count).clamp(1, height.max(1));
        let max = count.saturating_sub(height);
        let start = if max == 0 {
            0
        } else {
            self.file_state.offset() * (height - size) / max
        };
        (start as u16, size as u16)
    }

    fn drag_scroll(&mut self, row: u16, grab: u16) {
        let (_, size) = self.thumb();
        let travel = self.scrollbar.height.saturating_sub(size);
        let position = row
            .saturating_sub(self.scrollbar.y)
            .saturating_sub(grab)
            .min(travel);
        let max = self
            .tree
            .len()
            .saturating_sub(usize::from(self.scrollbar.height));
        *self.file_state.offset_mut() = if travel == 0 {
            0
        } else {
            usize::from(position) * max / usize::from(travel)
        };
    }

    fn reload_config(&mut self) {
        match crate::config::load(&self.opts.repo, self.opts.config.as_deref()) {
            Ok(config) => {
                self.opts.icons = self.opts.icon_override.unwrap_or(config.ui.icons);
                let languages = if self.opts.no_syntax {
                    Vec::new()
                } else {
                    config.syntax.languages
                };
                if self.opts.syntax_languages != languages {
                    self.opts.syntax_languages = languages;
                    self.highlights = Highlighter::new(self.opts.syntax_languages.clone());
                }
                let auto = !self.opts.no_auto_refresh && config.refresh.auto && !self.opts.demo;
                if self.opts.auto_refresh != auto {
                    self.opts.auto_refresh = auto;
                    self.pending_snapshot = None;
                    self.request_refresh();
                }
                self.status =
                    "Reloaded configuration. Startup update preferences apply next launch.".into();
            }
            Err(error) => self.status = format!("Config reload failed: {error}"),
        }
    }

    fn reload(&mut self) {
        self.pending_snapshot = None;
        self.manual_refresh = true;
        self.status = "Refreshing…".into();
        self.request_refresh();
    }

    fn request_refresh(&mut self) {
        self.refresher.request(
            Parameters {
                repo: self.opts.repo.clone(),
                staged: self.opts.staged,
                base: self.opts.base.clone(),
                paths: self.opts.paths.clone(),
                demo: self.opts.demo,
            },
            self.opts.auto_refresh,
        );
    }

    fn apply_snapshot(&mut self, snapshot: Snapshot) {
        let tree_path = self.tree.get(self.tree_cursor).map(|r| r.path.clone());
        let name = self.files.get(self.selected).map(|f| f.name.clone());
        let selected = snapshot
            .files
            .iter()
            .position(|f| Some(&f.name) == name.as_ref())
            .unwrap_or(self.selected.min(snapshot.files.len().saturating_sub(1)));
        self.highlights.invalidate();
        self.files = snapshot.files.into_iter().map(Arc::new).collect();
        self.selected = selected;
        if let Some(file) = self.files.get_mut(selected) {
            Arc::make_mut(file).load_current();
        }
        self.rebuild_tree();
        self.tree_cursor = self
            .tree
            .iter()
            .position(|r| Some(&r.path) == tree_path.as_ref())
            .or_else(|| self.tree.iter().position(|r| r.file == Some(selected)))
            .unwrap_or(self.tree_cursor);
        self.clamp();
        self.cursor = self
            .cursor
            .min(self.row_count().saturating_sub(1))
            .max(self.top);
        if self.manual_refresh || self.status.starts_with("Refresh failed:") {
            self.status = "Refreshed".into();
        }
        self.manual_refresh = false;
    }

    fn refresh_tick(&mut self) -> bool {
        if self.setup.is_some() {
            return false;
        }
        if self.opts.auto_refresh && !self.refresher.started {
            self.request_refresh();
        }
        self.refresher.flush();
        let mut changed = false;
        if let Some(reply) = self.refresher.receive() {
            match reply.snapshot {
                Ok(snapshot) => self.pending_snapshot = Some(snapshot),
                Err(error) => {
                    self.status = format!("Refresh failed: {error}");
                    changed = true;
                }
            }
        }
        if self.previous.is_none() && self.picker.is_none() {
            if let Some(snapshot) = self.pending_snapshot.take() {
                self.apply_snapshot(snapshot);
                changed = true;
            }
        }
        changed
    }

    fn scroll(&mut self, amount: isize) {
        self.top = self.top.saturating_add_signed(amount);
        self.clamp();
        self.cursor = self
            .cursor
            .clamp(self.top, self.top + self.viewport.saturating_sub(1));
        self.cursor = self.cursor.min(self.row_count().saturating_sub(1));
    }

    fn row_count(&self) -> usize {
        self.files
            .get(self.selected)
            .map(|file| {
                if self.view == View::Current {
                    file.current.len().max(1)
                } else {
                    file.rows(self.view == View::Split).len()
                }
            })
            .unwrap_or(0)
    }

    fn move_cursor(&mut self, delta: isize) {
        self.cursor = self
            .cursor
            .saturating_add_signed(delta)
            .min(self.row_count().saturating_sub(1));
        if self.cursor < self.top {
            self.top = self.cursor;
        }
        if self.cursor >= self.top + self.viewport {
            self.top = self.cursor.saturating_sub(self.viewport.saturating_sub(1));
        }
    }

    fn inspect_previous(&mut self) {
        if let Some(change) = self
            .files
            .get(self.selected)
            .and_then(|file| file.change_at(self.cursor))
        {
            self.previous = Some(Previous {
                lines: change.previous.clone(),
                top: 0,
                x: 0,
            });
        } else {
            self.status =
                "This line is unchanged. Click a +, ~, or - marker to inspect a change.".into();
        }
    }

    fn clamp(&mut self) {
        let count = self.row_count();
        self.top = self.top.min(count.saturating_sub(self.viewport));
    }

    fn hunk(&mut self, forward: bool) {
        if let Some(file) = self.files.get(self.selected) {
            let hunks: Vec<usize> = if self.view == View::Current {
                file.changes.iter().map(|change| change.start).collect()
            } else {
                let rows = file.rows(self.view == View::Split);
                rows.iter()
                    .enumerate()
                    .filter(|(_, (a, _))| a.is_some_and(|l| l.text.starts_with("@@ ")))
                    .map(|(i, _)| i)
                    .collect()
            };
            let position = if self.view == View::Current {
                self.cursor
            } else {
                self.top
            };
            let next = if forward {
                hunks.into_iter().find(|n| *n > position)
            } else {
                hunks.into_iter().rev().find(|n| *n < position)
            };
            if let Some(n) = next {
                self.top = n;
                self.cursor = n;
            }
        }
    }

    fn matches(&self) -> Vec<usize> {
        let query = self.picker.as_deref().unwrap_or("").to_lowercase();
        self.files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.name.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect()
    }

    fn open_picker(&mut self) {
        self.picker = Some(String::new());
        self.pick = 0;
    }

    fn receive_update(&mut self, message: Message) {
        match message {
            Message::Checked(release) => self.update_offer = release,
            Message::Installed(tag) => {
                self.status = format!("Updated to {tag}. Restart tdv to use the new version.");
            }
            Message::Failed(error) => {
                self.status = format!("Update failed: {error}");
            }
        }
    }

    fn setup_action(&mut self, action: SetupAction) -> bool {
        match action {
            SetupAction::Quit => return true,
            SetupAction::Skip => self.setup = None,
            SetupAction::Save => {
                let Some(setup) = &mut self.setup else {
                    return false;
                };
                match setup.target.save(&setup.config, setup.replace) {
                    Ok(()) => {
                        self.opts.syntax_languages = if self.opts.no_syntax {
                            Vec::new()
                        } else {
                            setup.config.syntax.languages.clone()
                        };
                        self.opts.icons = self.opts.icon_override.unwrap_or(setup.config.ui.icons);
                        self.opts.check_updates =
                            !self.opts.no_update_check && setup.config.updates.check_on_startup;
                        self.opts.auto_refresh = !self.opts.no_auto_refresh
                            && setup.config.refresh.auto
                            && !self.opts.demo;
                        self.highlights = Highlighter::new(self.opts.syntax_languages.clone());
                        self.updater = Updater::new(self.opts.check_updates);
                        self.status =
                            "Saved .tdv.toml; ignored locally. Use tdv --setup to reconfigure."
                                .into();
                        self.setup = None;
                    }
                    Err(error) => setup.error = Some(error.to_string()),
                }
            }
            SetupAction::None => {}
        }
        false
    }

    fn key(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return true;
        }
        if let Some(setup) = &mut self.setup {
            let action = setup.key(key);
            return self.setup_action(action);
        }
        if let Some(previous) = &mut self.previous {
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.previous = None,
                KeyCode::Down | KeyCode::Char('j') => {
                    previous.top = (previous.top + 1).min(previous.lines.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => previous.top = previous.top.saturating_sub(1),
                KeyCode::PageDown => {
                    previous.top = (previous.top + 10).min(previous.lines.len().saturating_sub(1))
                }
                KeyCode::PageUp => previous.top = previous.top.saturating_sub(10),
                KeyCode::Right | KeyCode::Char('l') => previous.x = previous.x.saturating_add(4),
                KeyCode::Left | KeyCode::Char('h') => previous.x = previous.x.saturating_sub(4),
                _ => {}
            }
            return false;
        }
        // Defer the prompt while the user is typing in a picker or reading help.
        if self.update_offer.is_some()
            && self.picker.is_none()
            && !self.help
            && self.previous.is_none()
        {
            match key.code {
                KeyCode::Char('y' | 'Y') if key.kind == KeyEventKind::Press => {
                    let release = self.update_offer.take().unwrap();
                    self.status = format!("Installing {}… You can keep browsing.", release.tag);
                    self.updater.install(release);
                    if !self.updater.is_busy() {
                        self.status =
                            "Could not start update worker; current version was kept.".into();
                    }
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Enter => {
                    self.update_offer = None
                }
                KeyCode::Char('q') => return true,
                _ => {}
            }
            return false;
        }
        if matches!(
            key.code,
            KeyCode::Modifier(ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift)
        ) {
            if key.kind != KeyEventKind::Press {
                return false;
            }
            let now = Instant::now();
            if self
                .last_shift
                .is_some_and(|t| now.duration_since(t) < Duration::from_millis(400))
            {
                self.open_picker();
                self.last_shift = None;
            } else {
                self.last_shift = Some(now);
            }
            return false;
        }
        self.last_shift = None;
        if self.help {
            self.help = false;
            return false;
        }
        if self.picker.is_some() {
            match key.code {
                KeyCode::Esc => self.picker = None,
                KeyCode::Enter => {
                    if let Some(n) = self.matches().get(self.pick).copied() {
                        self.select(n);
                        self.file_focus = false;
                    }
                    self.picker = None;
                }
                KeyCode::Down => {
                    self.pick = (self.pick + 1).min(self.matches().len().saturating_sub(1))
                }
                KeyCode::Up => self.pick = self.pick.saturating_sub(1),
                KeyCode::Backspace => {
                    self.picker.as_mut().unwrap().pop();
                    self.pick = 0;
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.picker.as_mut().unwrap().push(c);
                    self.pick = 0;
                }
                _ => {}
            }
            return false;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_picker()
            }
            KeyCode::Char('/') => self.open_picker(),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Tab | KeyCode::BackTab => {
                self.file_focus = !self.file_focus;
                self.sidebar = true;
            }
            KeyCode::Char('f') => {
                self.sidebar = !self.sidebar;
                self.file_focus = false;
            }
            KeyCode::Char('v') => {
                self.view = self.view.next();
                self.cursor = 0;
                self.top = 0;
            }
            KeyCode::Char('s') => {
                self.opts.staged = !self.opts.staged;
                self.reload();
            }
            KeyCode::Char('r') => self.reload(),
            KeyCode::Char('R') => self.reload_config(),
            KeyCode::Char(']') => self.select(self.selected.saturating_add(1)),
            KeyCode::Char('[') => self.select(self.selected.saturating_sub(1)),
            KeyCode::Char('n') => self.hunk(true),
            KeyCode::Char('p') => self.hunk(false),
            KeyCode::Enter => {
                if self.file_focus {
                    if self
                        .tree
                        .get(self.tree_cursor)
                        .is_some_and(|r| r.file.is_none())
                    {
                        self.toggle_directory();
                    } else {
                        self.file_focus = false;
                    }
                } else if self.view == View::Current {
                    self.inspect_previous();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.file_focus {
                    self.move_tree(1);
                } else if self.view == View::Current {
                    self.move_cursor(1);
                } else {
                    self.scroll(1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.file_focus {
                    self.move_tree(-1);
                } else if self.view == View::Current {
                    self.move_cursor(-1);
                } else {
                    self.scroll(-1);
                }
            }
            KeyCode::Right | KeyCode::Char('l') if self.file_focus => {
                if let Some(row) = self.tree.get(self.tree_cursor).filter(|r| r.file.is_none()) {
                    if self.collapsed.contains(&row.path) {
                        self.toggle_directory();
                    } else {
                        self.move_tree(1);
                    }
                }
            }
            KeyCode::Left | KeyCode::Char('h') if self.file_focus => {
                if let Some(row) = self.tree.get(self.tree_cursor) {
                    if row.file.is_none() && !self.collapsed.contains(&row.path) {
                        self.toggle_directory();
                    } else if let Some((parent, _)) = row.path.rsplit_once('/') {
                        if let Some(index) = self.tree.iter().position(|r| r.path == parent) {
                            self.tree_cursor = index;
                            self.ensure_tree_cursor();
                        }
                    }
                }
            }
            KeyCode::Right | KeyCode::Char('l') => self.x = self.x.saturating_add(4),
            KeyCode::Left | KeyCode::Char('h') => self.x = self.x.saturating_sub(4),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll(self.viewport as isize),
            KeyCode::PageUp => self.scroll(-(self.viewport as isize)),
            KeyCode::Home | KeyCode::Char('g') => {
                self.top = 0;
                self.cursor = 0;
                self.x = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.top = usize::MAX;
                self.clamp();
                self.cursor = self.row_count().saturating_sub(1);
            }
            _ => {}
        }
        false
    }

    fn mouse(&mut self, event: event::MouseEvent) {
        if event.kind == MouseEventKind::Up(MouseButton::Left) {
            self.drag = None;
            return;
        }
        if let Some(setup) = &mut self.setup {
            let action = setup.mouse(event);
            self.setup_action(action);
            return;
        }
        if let Some(previous) = &mut self.previous {
            match event.kind {
                MouseEventKind::ScrollDown => {
                    previous.top = (previous.top + 3).min(previous.lines.len().saturating_sub(1))
                }
                MouseEventKind::ScrollUp => previous.top = previous.top.saturating_sub(3),
                MouseEventKind::ScrollRight => previous.x = previous.x.saturating_add(4),
                MouseEventKind::ScrollLeft => previous.x = previous.x.saturating_sub(4),
                MouseEventKind::Down(MouseButton::Left) => self.previous = None,
                _ => {}
            }
            return;
        }
        if self.update_offer.is_some()
            && self.picker.is_none()
            && !self.help
            && self.previous.is_none()
        {
            if event.kind == MouseEventKind::Down(MouseButton::Left) {
                let point = (event.column, event.row).into();
                if self.update_yes.contains(point) {
                    self.key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
                } else if self.update_no.contains(point) {
                    self.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
                }
            }
            return;
        }
        if self.help || self.picker.is_some() {
            return;
        }
        if event.kind == MouseEventKind::Drag(MouseButton::Left) {
            match self.drag {
                Some(Drag::Resize) => {
                    self.sidebar_width = Some(
                        event
                            .column
                            .saturating_add(1)
                            .clamp(20, self.screen_width.saturating_sub(30).max(20)),
                    )
                }
                Some(Drag::Scroll { grab }) => self.drag_scroll(event.row, grab),
                None => {}
            }
            return;
        }
        let inside = |r: Rect| r.contains((event.column, event.row).into());
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if event.row == 0 => {
                if event.column < 14 {
                    self.view = self.view.next();
                    self.cursor = 0;
                    self.top = 0;
                } else if event.column < 30 {
                    self.opts.staged = !self.opts.staged;
                    self.reload();
                } else {
                    self.open_picker();
                }
            }
            MouseEventKind::Down(MouseButton::Left) if inside(self.sidebar_edge) => {
                self.drag = Some(Drag::Resize);
                self.last_click = None;
            }
            MouseEventKind::Down(MouseButton::Left) if inside(self.scrollbar) => {
                let (start, size) = self.thumb();
                let position = event.row - self.scrollbar.y;
                let grab = if position >= start && position < start + size {
                    position - start
                } else {
                    size / 2
                };
                self.drag = Some(Drag::Scroll { grab });
                self.drag_scroll(event.row, grab);
                self.last_click = None;
            }
            MouseEventKind::Down(MouseButton::Left) if inside(self.list_area) => {
                let n = self.file_state.offset() + usize::from(event.row - self.list_area.y);
                if let Some(row) = self.tree.get(n) {
                    let file = row.file;
                    self.tree_cursor = n;
                    self.file_focus = true;
                    if let Some(file) = file {
                        let double = self.last_click.is_some_and(|(i, t)| {
                            i == file && t.elapsed() < Duration::from_millis(400)
                        });
                        self.select(file);
                        if double {
                            self.sidebar = false;
                            self.file_focus = false;
                        }
                        self.last_click = Some((file, Instant::now()));
                    } else {
                        self.toggle_directory();
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) if inside(self.body_area) => {
                self.file_focus = false;
                if self.view == View::Current {
                    let clicked = self.top + usize::from(event.row - self.body_area.y);
                    if clicked >= self.row_count() {
                        return;
                    }
                    self.cursor = clicked;
                    if self
                        .files
                        .get(self.selected)
                        .is_some_and(|f| f.change_at(self.cursor).is_some())
                    {
                        self.inspect_previous();
                    }
                }
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp if inside(self.list_area) => {
                *self.file_state.offset_mut() = self.file_state.offset().saturating_add_signed(
                    if event.kind == MouseEventKind::ScrollDown {
                        3
                    } else {
                        -3
                    },
                );
                self.clamp_tree_scroll();
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp if inside(self.body_area) => {
                let down = event.kind == MouseEventKind::ScrollDown;
                if event.modifiers.contains(KeyModifiers::SHIFT) {
                    self.x = if down {
                        self.x.saturating_add(4)
                    } else {
                        self.x.saturating_sub(4)
                    };
                } else {
                    self.scroll(if down { 3 } else { -3 });
                }
            }
            MouseEventKind::ScrollRight => self.x = self.x.saturating_add(4),
            MouseEventKind::ScrollLeft => self.x = self.x.saturating_sub(4),
            _ => {}
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        self.update_yes = Rect::default();
        self.update_no = Rect::default();
        let area = frame.area();
        self.screen_width = area.width;
        self.list_area = Rect::default();
        self.sidebar_edge = Rect::default();
        self.scrollbar = Rect::default();
        frame.render_widget(Block::default().style(Style::default().bg(BG).fg(FG)), area);
        if area.width < 30 || area.height < 6 {
            frame.render_widget(Paragraph::new("Resize to at least 30 × 6. q quits."), area);
            return;
        }
        let sections = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
        let mode = self.view.name();
        let source = if self.opts.demo {
            "Demo"
        } else if self.opts.staged {
            "Staged"
        } else {
            "Unstaged"
        };
        frame.render_widget(
            Paragraph::new(format!(
                " [v] {mode:<7}  [s] {source:<8}  [Ctrl+P] Find file   tdv"
            ))
            .style(Style::default().bg(Color::Rgb(34, 43, 57)).fg(ACCENT)),
            sections[0],
        );
        let show_sidebar = self.sidebar && area.width >= 65;
        let columns = Layout::horizontal([
            Constraint::Length(if show_sidebar {
                self.sidebar_width
                    .unwrap_or((area.width / 4).clamp(20, 38))
                    .clamp(20, area.width.saturating_sub(30).max(20))
            } else {
                0
            }),
            Constraint::Min(1),
        ])
        .split(sections[1]);
        self.list_area = Rect::default();
        if show_sidebar {
            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" Files · {} ", self.files.len()))
                .border_style(Style::default().fg(if self.file_focus { ACCENT } else { MUTED }));
            self.list_area = block.inner(columns[0]);
            self.sidebar_edge = Rect::new(
                columns[0].right() - 1,
                self.list_area.y,
                1,
                self.list_area.height,
            );
            self.clamp_tree_scroll();
            let items: Vec<ListItem> = self
                .tree
                .iter()
                .enumerate()
                .skip(self.file_state.offset())
                .take(usize::from(self.list_area.height))
                .map(|(index, row)| {
                    let indent = "  ".repeat(row.depth);
                    let label = if row.file.is_some() {
                        let name = safe_name(&row.label);
                        if self.opts.icons {
                            format!("{indent}{} {name}", file_tree::icon(&row.label))
                        } else {
                            format!("{indent}{name}")
                        }
                    } else {
                        format!(
                            "{indent}{} {}/",
                            if self.collapsed.contains(&row.path) {
                                "▸"
                            } else {
                                "▾"
                            },
                            safe_name(&row.label)
                        )
                    };
                    let active = if self.file_focus {
                        index == self.tree_cursor
                    } else {
                        row.file == Some(self.selected)
                    };
                    ListItem::new(label).style(if active {
                        Style::default().bg(Color::Rgb(42, 61, 78)).fg(ACCENT)
                    } else {
                        Style::default()
                    })
                })
                .collect();
            frame.render_widget(block, columns[0]);
            let mut content = self.list_area;
            if self.tree.len() > usize::from(content.height) && content.height > 0 {
                self.scrollbar = Rect::new(content.right() - 1, content.y, 1, content.height);
                content.width = content.width.saturating_sub(1);
            }
            frame.render_widget(List::new(items), content);
            if self.scrollbar.height > 0 {
                let (start, size) = self.thumb();
                for y in 0..self.scrollbar.height {
                    frame.render_widget(
                        Paragraph::new(if y >= start && y < start + size {
                            "█"
                        } else {
                            "│"
                        })
                        .style(Style::default().fg(MUTED)),
                        Rect::new(self.scrollbar.x, self.scrollbar.y + y, 1, 1),
                    );
                }
            }
        }
        let title = self
            .files
            .get(self.selected)
            .map(|f| format!(" {} ", safe_name(&f.name)))
            .unwrap_or_else(|| " Changes ".into());
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(if self.file_focus { MUTED } else { ACCENT }));
        self.body_area = block.inner(columns[1]);
        frame.render_widget(block, columns[1]);
        self.viewport = self.body_area.height as usize;
        if self.files.is_empty() {
            frame.render_widget(
                Paragraph::new(
                    "No changes in this view.\nPress s to switch staged/unstaged, or r to refresh.",
                )
                .style(Style::default().fg(MUTED)),
                self.body_area,
            );
        } else {
            let file = &self.files[self.selected];
            let colors = self.highlights.get(self.selected);
            let rows = if self.view == View::Current {
                Vec::new()
            } else {
                file.rows(self.view == View::Split)
            };
            if self.view == View::Current {
                if file.current.is_empty() {
                    frame.render_widget(
                        Paragraph::new(file.note.as_deref().unwrap_or("Empty file."))
                            .style(Style::default().fg(MUTED)),
                        self.body_area,
                    );
                } else {
                    let panes = Layout::horizontal([Constraint::Length(10), Constraint::Min(1)])
                        .split(self.body_area);
                    let mut gutters = Vec::new();
                    let mut contents = Vec::new();
                    for line in file.current.iter().skip(self.top).take(self.viewport) {
                        let change = file.change_at(line.index).map(|c| c.kind);
                        let (sign, color, bg) = match change {
                            Some(ChangeKind::Added) => {
                                ("+", Color::LightGreen, Color::Rgb(23, 46, 35))
                            }
                            Some(ChangeKind::Modified) => {
                                ("~", Color::Yellow, Color::Rgb(39, 39, 27))
                            }
                            Some(ChangeKind::Removed) => {
                                ("-", Color::LightRed, Color::Rgb(52, 30, 37))
                            }
                            None => (" ", MUTED, BG),
                        };
                        let selected = line.index == self.cursor && !self.file_focus;
                        gutters.push(Line::styled(
                            format!(
                                "{} {:>5} {sign} ",
                                if selected { "›" } else { " " },
                                line.index + 1
                            ),
                            Style::default().fg(color).bg(if selected {
                                Color::Rgb(42, 61, 78)
                            } else {
                                BG
                            }),
                        ));
                        let tokens = colors
                            .and_then(|c| c.current.get(line.index))
                            .and_then(|t| t.as_ref());
                        let spans = tokens.cloned().unwrap_or_else(|| {
                            vec![Span::styled(line.text.clone(), Style::default().fg(FG))]
                        });
                        contents.push(Line::from(spans).style(Style::default().bg(bg)));
                    }
                    frame.render_widget(Paragraph::new(gutters), panes[0]);
                    frame.render_widget(Paragraph::new(contents).scroll((0, self.x)), panes[1]);
                }
            } else if self.view == View::Split {
                let panes = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(self.body_area);
                for (side, pane) in panes.iter().enumerate() {
                    let lines: Vec<Line> = rows
                        .iter()
                        .skip(self.top)
                        .take(self.viewport)
                        .map(|(a, b)| {
                            code_line(if side == 0 { *a } else { *b }, side, true, colors)
                        })
                        .collect();
                    frame.render_widget(Paragraph::new(lines).scroll((0, self.x)), *pane);
                }
            } else {
                let lines: Vec<Line> = rows
                    .iter()
                    .skip(self.top)
                    .take(self.viewport)
                    .map(|(a, _)| code_line(*a, 0, false, colors))
                    .collect();
                frame.render_widget(Paragraph::new(lines).scroll((0, self.x)), self.body_area);
            }
        }
        let footer = if self.status.is_empty() {
            format!(
                " {}/{} · line {} · {} · + new ~ changed - deleted · Enter previous · v view · ? help · q quit",
                if self.files.is_empty() {
                    0
                } else {
                    self.selected + 1
                },
                self.files.len(),
                self.cursor + 1,
                if self.opts.auto_refresh { "live" } else { "r refresh" }
            )
        } else {
            format!(" {} · ? help · q quit", safe_name(&self.status))
        };
        frame.render_widget(
            Paragraph::new(footer).style(Style::default().fg(MUTED)),
            sections[2],
        );
        if let Some(setup) = &mut self.setup {
            setup.draw(frame);
        } else if let Some(previous) = &self.previous {
            let popup = centered(area, 100, 28);
            frame.render_widget(Clear, popup);
            let block = Block::bordered()
                .title(" Previous code ")
                .style(Style::default().bg(BG).fg(FG));
            let inner = block.inner(popup);
            frame.render_widget(block, popup);
            let parts = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
            if previous.lines.is_empty() {
                frame.render_widget(
                    Paragraph::new("These lines are new. There is no previous code."),
                    parts[0],
                );
            } else {
                let colors = self.highlights.get(self.selected);
                let lines: Vec<_> = previous
                    .lines
                    .iter()
                    .skip(previous.top)
                    .take(parts[0].height as usize)
                    .map(|line| code_line(Some(line), 0, true, colors))
                    .collect();
                frame.render_widget(Paragraph::new(lines).scroll((0, previous.x)), parts[0]);
            }
            frame.render_widget(
                Paragraph::new("↑↓ scroll · ←→ pan · Esc / Enter / click closes")
                    .style(Style::default().fg(MUTED)),
                parts[1],
            );
        } else if self.update_offer.is_some()
            && self.picker.is_none()
            && !self.help
            && self.previous.is_none()
        {
            let release = self.update_offer.as_ref().unwrap();
            let popup = centered(area, 68, 9);
            if popup.height >= 8 {
                self.update_yes =
                    Rect::new(popup.x + 1, popup.y + 5, popup.width.saturating_sub(2), 1);
                self.update_no = Rect::new(
                    popup.x + 1,
                    popup.y + 6,
                    25.min(popup.width.saturating_sub(2)),
                    1,
                );
            }
            frame.render_widget(Clear, popup);
            frame.render_widget(Paragraph::new(format!(
                "A newer release is available: {} → {}\n\nInstall the update now?\n\n[y] Download, verify, and install\n[n / Esc / Enter] Later     [q] Quit",
                env!("CARGO_PKG_VERSION"), safe_name(&release.tag)))
                .block(Block::bordered().title(" Update available "))
                .style(Style::default().bg(BG).fg(FG)), popup);
        } else if self.help {
            let popup = centered(area, 74, 22);
            frame.render_widget(Clear, popup);
            frame.render_widget(Paragraph::new("Mouse\n  Click folders to collapse; click files to select\n  Drag the file pane border to resize; drag its scrollbar to scroll\n  Click changed lines for previous code; wheel scrolls; Shift+wheel or horizontal wheel pans\n  Click the top bar to change view, source, or find files\n\nKeyboard\n  ↑↓ / j k       Scroll (or select files when focused)\n  ←→ / h l       Pan code / collapse or expand folders\n  Tab / Enter    Switch focus / inspect previous code\n  [ ]            Previous / next file\n  n p            Next / previous hunk\n  PgUp PgDn      Scroll one page\n  Home End / g G First / last page\n  v / f          Cycle current/unified/split / sidebar\n  s / r          Toggle staged changes / refresh\n  Shift+R        Reload config without restarting\n  Ctrl+P or /    Find file (also double-Shift if supported)\n  q / Ctrl+C     Quit\n\nPress any key to close.")
                .block(Block::bordered().title(" Shortcuts ")).style(Style::default().bg(BG).fg(FG)), popup);
        } else if let Some(query) = &self.picker {
            let popup = centered(area, 70, 18);
            frame.render_widget(Clear, popup);
            let block = Block::bordered()
                .title(format!(" Find file: {query}▏ "))
                .border_style(Style::default().fg(ACCENT))
                .style(Style::default().bg(BG).fg(FG));
            let matches = self.matches();
            let items: Vec<ListItem> = matches
                .iter()
                .map(|i| ListItem::new(safe_name(&self.files[*i].name)))
                .collect();
            let mut state = ListState::default().with_selected(Some(self.pick));
            frame.render_stateful_widget(
                List::new(items)
                    .block(block)
                    .highlight_style(Style::default().bg(Color::Rgb(42, 61, 78)))
                    .highlight_symbol("› "),
                popup,
                &mut state,
            );
        }
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn safe_name(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '�' } else { c })
        .collect()
}

fn code_line(
    line: Option<&diff::Line>,
    side: usize,
    split: bool,
    colors: Option<&Colors>,
) -> Line<'static> {
    let Some(line) = line else {
        return Line::raw("");
    };
    let (sign, color, bg) = match line.kind {
        Kind::Added => ("+", Color::Rgb(150, 222, 166), Color::Rgb(23, 46, 35)),
        Kind::Removed => ("-", Color::Rgb(242, 155, 155), Color::Rgb(52, 30, 37)),
        Kind::Header => (" ", ACCENT, Color::Rgb(26, 36, 49)),
        Kind::Context => (" ", FG, BG),
    };
    let number = |n: Option<usize>| {
        n.map(|n| format!("{n:>5}"))
            .unwrap_or_else(|| "     ".into())
    };
    let gutter = if split {
        format!(
            "{} {sign} ",
            number(if side == 0 { line.old } else { line.new })
        )
    } else {
        format!("{} {} {sign} ", number(line.old), number(line.new))
    };
    let syntax_side = if split {
        side
    } else if line.kind == Kind::Removed {
        0
    } else {
        1
    };
    let syntax = colors
        .and_then(|colors| colors.lines.get(line.index))
        .and_then(|pair| pair[syntax_side].as_ref());
    let mut spans = vec![Span::styled(gutter, Style::default().fg(MUTED))];
    if let Some(tokens) = syntax {
        spans.extend(tokens.iter().cloned());
    } else {
        spans.push(Span::styled(line.text.clone(), Style::default().fg(color)));
    }
    Line::from(spans).style(Style::default().bg(bg))
}

pub fn run(opts: Options, snapshot: Snapshot) -> io::Result<()> {
    let mut terminal = ratatui::try_init()?;
    let _cleanup = Cleanup;
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture, PopKeyboardEnhancementFlags);
        old_hook(info);
    }));
    if !opts.no_mouse {
        execute!(stdout(), EnableMouseCapture)?;
    }
    // Unsupported terminals ignore these progressive enhancement sequences.
    execute!(
        stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        )
    )?;
    let mut app = App::new(opts, snapshot);
    let mut dirty = true;
    loop {
        if dirty {
            terminal.draw(|frame| app.draw(frame))?;
            dirty = false;
        }
        if app.refresh_tick() {
            dirty = true;
        }
        // First paint always precedes the first syntax request. Neither sending
        // requests nor collecting colors waits for the background worker.
        if app.setup.is_none() {
            if let Some(file) = app.files.get(app.selected) {
                app.highlights.request(app.selected, file);
            }
            app.updater.start_check();
        }
        if app.highlights.receive() {
            dirty = true;
        }
        if let Some(message) = app.updater.receive() {
            app.receive_update(message);
            dirty = true;
        }
        // Paint new prompts before processing their confirmation keys.
        if dirty {
            continue;
        }
        if (app.highlights.is_enabled() || app.updater.is_busy() || app.refresher.is_active())
            && !event::poll(Duration::from_millis(32))?
        {
            continue;
        }
        dirty = true;
        match event::read()? {
            Event::Key(key) => {
                if app.key(key) {
                    break;
                }
            }
            Event::Mouse(mouse) => app.mouse(mouse),
            Event::Resize(_, _) => {
                app.top = 0;
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_refresh_preserves_selection_and_viewport_and_defers_previews() {
        let patch = format!(
            "diff --git a/code.rs b/code.rs\n--- a/code.rs\n+++ b/code.rs\n@@ -1,40 +1,40 @@\n{}",
            (0..40).map(|n| format!(" line {n}\n")).collect::<String>()
        );
        let mut app = App::new(Options::default(), Snapshot::from_patch(&patch));
        app.viewport = 10;
        app.top = 15;
        app.cursor = 18;
        app.x = 8;
        let new_patch = format!("{}{}", diff::DEMO, patch);
        app.previous = Some(Previous {
            lines: vec![],
            top: 0,
            x: 0,
        });
        app.pending_snapshot = Some(Snapshot::from_patch(&new_patch));
        assert!(!app.refresh_tick());
        assert_eq!(app.files.len(), 1);
        app.previous = None;
        assert!(app.refresh_tick());
        assert_eq!(app.files[app.selected].name, "code.rs");
        assert_eq!((app.top, app.cursor, app.x), (15, 18, 8));
    }
    #[test]
    fn current_view_hides_old_code_and_opens_previous_with_keyboard_and_mouse() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut app = App::new(Options::default(), Snapshot::from_patch(diff::DEMO));
        assert!(app.view == View::Current);
        let mut terminal = Terminal::new(TestBackend::new(140, 24)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(screen.contains("let name = name.trim()"));
        assert!(!screen.contains("format!(\"Hello, {}\", name)"));
        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app
            .previous
            .as_ref()
            .unwrap()
            .lines
            .iter()
            .any(|l| l.text.contains("Hello, {}")));
        terminal.draw(|frame| app.draw(frame)).unwrap();
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.previous.is_none());
        app.mouse(event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            row: app.body_area.y + 1,
            column: app.body_area.x + 1,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.previous.is_some());
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        for expected in [View::Unified, View::Split, View::Current] {
            app.key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
            assert!(app.view == expected);
        }
    }
    #[test]
    fn update_prompt_requires_explicit_consent_and_defers_while_typing() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut app = App::new(Options::default(), Snapshot::from_patch(diff::DEMO));
        let release = Release {
            tag: "v0.3.0".into(),
            asset: "tdv-test.tar.gz".into(),
        };
        app.open_picker();
        app.receive_update(Message::Checked(Some(release.clone())));
        app.key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert_eq!(app.picker.as_deref(), Some("y"));
        assert!(!app.updater.is_busy());
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(screen.contains("Update available"));
        assert!(screen.contains("v0.3.0"));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.update_offer.is_none());
        assert!(!app.updater.is_busy());
        app.receive_update(Message::Checked(Some(release)));
        app.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(app.update_offer.is_none());
        app.receive_update(Message::Installed("v0.3.0".into()));
        assert!(app.status.contains("Restart"));
        app.receive_update(Message::Failed("checksum mismatch".into()));
        assert!(app.status.contains("checksum mismatch"));
    }
    #[test]
    fn shortcuts_picker_and_bounds() {
        let mut app = App::new(Options::default(), Snapshot::from_patch(diff::DEMO));
        app.key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
        assert_eq!(app.selected, 1);
        app.select(100);
        assert_eq!(app.selected, 1);
        app.open_picker();
        for c in "greeting".chars() {
            app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.selected, 0);
        app.scroll(100);
        assert!(app.top < 100);
    }
    #[test]
    fn double_shift_and_mouse_navigation() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut app = App::new(Options::default(), Snapshot::from_patch(diff::DEMO));
        let shift = KeyEvent::new(
            KeyCode::Modifier(ModifierKeyCode::LeftShift),
            KeyModifiers::SHIFT,
        );
        app.key(shift);
        assert!(app.picker.is_none());
        app.key(shift);
        assert!(app.picker.is_some());
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let click = event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: app.list_area.x + 1,
            row: app.list_area.y + 2,
            modifiers: KeyModifiers::NONE,
        };
        app.mouse(click);
        assert_eq!(app.selected, 1);
        assert!(app.file_focus);
        app.mouse(click);
        assert!(!app.sidebar);
        assert!(!app.file_focus);
        app.mouse(event::MouseEvent {
            kind: MouseEventKind::ScrollRight,
            ..click
        });
        assert_eq!(app.x, 4);
    }
    #[test]
    fn folders_collapse_survive_refresh_and_picker_reveals_selection() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut app = App::new(Options::default(), Snapshot::from_patch(diff::DEMO));
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let click = event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: app.list_area.x,
            row: app.list_area.y,
            modifiers: KeyModifiers::NONE,
        };
        app.mouse(click);
        assert!(app.collapsed.contains("src"));
        assert_eq!(app.tree.len(), 2);
        assert_eq!(app.selected, 0); // Closing a folder keeps the current diff open.
        app.apply_snapshot(Snapshot::from_patch(diff::DEMO));
        assert!(app.collapsed.contains("src"));
        assert_eq!(app.tree.len(), 2);
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(!app.collapsed.contains("src"));
        app.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(app.collapsed.contains("src"));
        app.open_picker();
        app.picker = Some("greeting".into());
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!app.collapsed.contains("src"));
        assert_eq!(app.tree[app.tree_cursor].file, Some(0));
        app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.tree[app.tree_cursor].path, "src");
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.collapsed.contains("src"));
        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.files[app.selected].name, "README.md");
    }

    #[test]
    fn dragging_border_and_scrollbar_and_wheel_keep_diff_selection() {
        use ratatui::{backend::TestBackend, Terminal};
        let patch = (0..50).map(|i| format!(
            "diff --git a/src/file{i:02}.rs b/src/file{i:02}.rs\n--- a/src/file{i:02}.rs\n+++ b/src/file{i:02}.rs\n@@ -1 +1 @@\n-old\n+new\n"
        )).collect::<String>();
        let mut app = App::new(Options::default(), Snapshot::from_patch(&patch));
        let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let mut mouse = event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: app.sidebar_edge.x,
            row: app.sidebar_edge.y,
            modifiers: KeyModifiers::NONE,
        };
        app.mouse(mouse);
        mouse.kind = MouseEventKind::Drag(MouseButton::Left);
        mouse.column = 39;
        app.mouse(mouse);
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(app.list_area.width, 38);
        mouse.kind = MouseEventKind::Up(MouseButton::Left);
        app.mouse(mouse);
        assert!(app.drag.is_none());
        mouse.column = app.scrollbar.x;
        mouse.row = app.scrollbar.y;
        mouse.kind = MouseEventKind::Down(MouseButton::Left);
        app.mouse(mouse);
        mouse.kind = MouseEventKind::Drag(MouseButton::Left);
        mouse.row = 100; // Dragging outside the pane clamps at the end.
        app.mouse(mouse);
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(
            app.file_state.offset(),
            app.tree.len() - usize::from(app.list_area.height)
        );
        assert_eq!(app.selected, 0);
        mouse.kind = MouseEventKind::Up(MouseButton::Left);
        app.mouse(mouse);
        mouse.kind = MouseEventKind::ScrollUp;
        mouse.row = app.list_area.y;
        mouse.column = app.list_area.x;
        let offset = app.file_state.offset();
        app.mouse(mouse);
        assert_eq!(app.file_state.offset(), offset - 3);
        assert_eq!(app.selected, 0);
        mouse.kind = MouseEventKind::Down(MouseButton::Left);
        let expected = app.tree[app.file_state.offset()].file.unwrap();
        app.mouse(mouse);
        assert_eq!(app.selected, expected);
        app.collapsed.insert("src".into());
        app.rebuild_tree();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(app.file_state.offset(), 0);
        assert_eq!(app.scrollbar.height, 0);
        terminal.backend_mut().resize(10, 3);
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(app.sidebar_edge.width, 0);
    }

    #[test]
    fn config_reload_applies_live_and_retains_settings_on_error() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.toml");
        // Existing configuration needs no migration and defaults to no icons.
        std::fs::write(&path, "[syntax]\nlanguages = []\n[refresh]\nauto = false\n").unwrap();
        let mut app = App::new(
            Options {
                config: Some(path.clone()),
                ..Options::default()
            },
            Snapshot::from_patch(diff::DEMO),
        );
        app.key(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT));
        assert!(!app.opts.icons);
        std::fs::write(
            &path,
            "[ui]\nicons = true\n[syntax]\nlanguages = [\"rust\"]\n[refresh]\nauto = false\n",
        )
        .unwrap();
        app.key(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT));
        assert!(app.opts.icons);
        assert_eq!(app.opts.syntax_languages, ["rust"]);
        assert!(app.status.starts_with("Reloaded configuration"));
        std::fs::write(&path, "[ui]\nicons = invalid\n").unwrap();
        app.reload_config();
        assert!(app.status.starts_with("Config reload failed:"));
        assert!(app.opts.icons);
        assert_eq!(app.opts.syntax_languages, ["rust"]);
        app.opts.icon_override = Some(false);
        app.opts.no_syntax = true;
        app.opts.no_auto_refresh = true;
        std::fs::write(
            &path,
            "[ui]\nicons = true\n[syntax]\nlanguages = [\"rust\"]\n",
        )
        .unwrap();
        app.reload_config();
        assert!(!app.opts.icons);
        assert!(app.opts.syntax_languages.is_empty());
        assert!(!app.opts.auto_refresh);
    }

    #[test]
    fn render_small_and_normal_terminals() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut app = App::new(Options::default(), Snapshot::from_patch(diff::DEMO));
        for (w, h) in [(10, 3), (30, 6), (80, 24), (160, 40)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|frame| app.draw(frame)).unwrap();
            app.open_picker();
            terminal.draw(|frame| app.draw(frame)).unwrap();
            app.picker = None;
            app.help = true;
            terminal.draw(|frame| app.draw(frame)).unwrap();
            app.help = false;
        }
    }
}
