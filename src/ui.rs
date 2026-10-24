use crate::{
    diff::{self, FileDiff, Kind},
    highlight::{Colors, Highlighter},
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
    io::{self, stdout},
    sync::Arc,
    time::{Duration, Instant},
};

const BG: Color = Color::Rgb(19, 23, 31);
const FG: Color = Color::Rgb(211, 220, 231);
const MUTED: Color = Color::Rgb(132, 148, 170);
const ACCENT: Color = Color::Rgb(114, 203, 231);

struct Cleanup;
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = execute!(stdout(), DisableMouseCapture, PopKeyboardEnhancementFlags);
        ratatui::restore();
    }
}

struct App {
    opts: Options,
    files: Vec<Arc<FileDiff>>,
    highlights: Highlighter,
    updater: Updater,
    update_offer: Option<Release>,
    update_yes: Rect,
    update_no: Rect,
    selected: usize,
    top: usize,
    x: u16,
    split: bool,
    sidebar: bool,
    file_focus: bool,
    file_state: ListState,
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
    fn new(opts: Options, patch: &str) -> Self {
        let highlights = Highlighter::new(opts.syntax_languages.clone());
        let updater = Updater::new(!opts.no_update_check);
        Self {
            opts,
            files: diff::parse(patch).into_iter().map(Arc::new).collect(),
            highlights,
            updater,
            update_offer: None,
            update_yes: Rect::default(),
            update_no: Rect::default(),
            selected: 0,
            top: 0,
            x: 0,
            split: true,
            sidebar: true,
            file_focus: false,
            file_state: ListState::default(),
            picker: None,
            pick: 0,
            help: false,
            status: String::new(),
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
        self.file_state.select(Some(self.selected));
    }

    fn reload(&mut self) {
        match self.opts.patch() {
            Ok(patch) => {
                let name = self.files.get(self.selected).map(|f| f.name.clone());
                self.highlights.invalidate();
                self.files = diff::parse(&patch).into_iter().map(Arc::new).collect();
                let selected = self
                    .files
                    .iter()
                    .position(|f| Some(&f.name) == name.as_ref())
                    .unwrap_or(0);
                self.select(selected);
                self.status = "Refreshed".into();
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    fn scroll(&mut self, amount: isize) {
        self.top = self.top.saturating_add_signed(amount);
        self.clamp();
    }

    fn clamp(&mut self) {
        let count = self
            .files
            .get(self.selected)
            .map(|f| f.rows(self.split).len())
            .unwrap_or(0);
        self.top = self.top.min(count.saturating_sub(self.viewport));
    }

    fn hunk(&mut self, forward: bool) {
        if let Some(file) = self.files.get(self.selected) {
            let rows = file.rows(self.split);
            let hunks: Vec<usize> = rows
                .iter()
                .enumerate()
                .filter(|(_, (a, _))| a.is_some_and(|l| l.text.starts_with("@@ ")))
                .map(|(i, _)| i)
                .collect();
            let next = if forward {
                hunks.into_iter().find(|n| *n > self.top)
            } else {
                hunks.into_iter().rev().find(|n| *n < self.top)
            };
            if let Some(n) = next {
                self.top = n;
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

    fn key(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return true;
        }
        // Defer the prompt while the user is typing in a picker or reading help.
        if self.update_offer.is_some() && self.picker.is_none() && !self.help {
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
                self.split = !self.split;
                self.top = 0;
            }
            KeyCode::Char('s') => {
                self.opts.staged = !self.opts.staged;
                self.reload();
            }
            KeyCode::Char('r') => self.reload(),
            KeyCode::Char(']') => self.select(self.selected.saturating_add(1)),
            KeyCode::Char('[') => self.select(self.selected.saturating_sub(1)),
            KeyCode::Char('n') => self.hunk(true),
            KeyCode::Char('p') => self.hunk(false),
            KeyCode::Enter => self.file_focus = false,
            KeyCode::Down | KeyCode::Char('j') => {
                if self.file_focus {
                    self.select(self.selected.saturating_add(1));
                } else {
                    self.scroll(1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.file_focus {
                    self.select(self.selected.saturating_sub(1));
                } else {
                    self.scroll(-1);
                }
            }
            KeyCode::Right | KeyCode::Char('l') => self.x = self.x.saturating_add(4),
            KeyCode::Left | KeyCode::Char('h') => self.x = self.x.saturating_sub(4),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll(self.viewport as isize),
            KeyCode::PageUp => self.scroll(-(self.viewport as isize)),
            KeyCode::Home | KeyCode::Char('g') => {
                self.top = 0;
                self.x = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.top = usize::MAX;
                self.clamp();
            }
            _ => {}
        }
        false
    }

    fn mouse(&mut self, event: event::MouseEvent) {
        if self.update_offer.is_some() && self.picker.is_none() && !self.help {
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
        let inside = |r: Rect| r.contains((event.column, event.row).into());
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) if event.row == 0 => {
                if event.column < 14 {
                    self.split = !self.split;
                    self.top = 0;
                } else if event.column < 30 {
                    self.opts.staged = !self.opts.staged;
                    self.reload();
                } else {
                    self.open_picker();
                }
            }
            MouseEventKind::Down(MouseButton::Left) if inside(self.list_area) => {
                let n = self.file_state.offset() + usize::from(event.row - self.list_area.y);
                if n < self.files.len() {
                    let double = self
                        .last_click
                        .is_some_and(|(i, t)| i == n && t.elapsed() < Duration::from_millis(400));
                    self.select(n);
                    self.file_focus = true;
                    if double {
                        self.sidebar = false;
                        self.file_focus = false;
                    }
                    self.last_click = Some((n, Instant::now()));
                }
            }
            MouseEventKind::Down(MouseButton::Left) if inside(self.body_area) => {
                self.file_focus = false
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp if inside(self.list_area) => {
                if event.kind == MouseEventKind::ScrollDown {
                    self.select(self.selected.saturating_add(1));
                } else {
                    self.select(self.selected.saturating_sub(1));
                }
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
        let mode = if self.split { "Split" } else { "Unified" };
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
                (area.width / 4).clamp(20, 38)
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
            let items: Vec<ListItem> = self
                .files
                .iter()
                .map(|f| {
                    let (a, d) = f.counts();
                    ListItem::new(format!("{} +{a} -{d}", safe_name(&f.name)))
                })
                .collect();
            self.file_state.select(if self.files.is_empty() {
                None
            } else {
                Some(self.selected)
            });
            frame.render_stateful_widget(
                List::new(items)
                    .block(block)
                    .highlight_style(Style::default().bg(Color::Rgb(42, 61, 78)).fg(ACCENT))
                    .highlight_symbol("› "),
                columns[0],
                &mut self.file_state,
            );
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
            frame.render_widget(Paragraph::new("No changes in this view.\nPress s to switch staged/unstaged, or r to refresh.\nUntracked files are not included.").style(Style::default().fg(MUTED)), self.body_area);
        } else {
            let file = &self.files[self.selected];
            let colors = self.highlights.get(self.selected);
            let rows = file.rows(self.split);
            if self.split {
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
                " {}/{} · row {} · Tab focus · n/p hunk · v view · f files · ? help · q quit",
                if self.files.is_empty() {
                    0
                } else {
                    self.selected + 1
                },
                self.files.len(),
                self.top + 1
            )
        } else {
            format!(" {} · ? help · q quit", safe_name(&self.status))
        };
        frame.render_widget(
            Paragraph::new(footer).style(Style::default().fg(MUTED)),
            sections[2],
        );
        if self.update_offer.is_some() && self.picker.is_none() && !self.help {
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
            frame.render_widget(Paragraph::new("Mouse\n  Click files to select; double-click to expand the diff\n  Wheel scrolls; Shift+wheel or horizontal wheel pans\n  Click the top bar to change view, source, or find files\n\nKeyboard\n  ↑↓ / j k       Scroll (or select files when focused)\n  ←→ / h l       Pan horizontally\n  Tab / Enter    Switch focus / focus diff\n  [ ]            Previous / next file\n  n p            Next / previous hunk\n  PgUp PgDn      Scroll one page\n  Home End / g G First / last page\n  v / f          Toggle split view / file sidebar\n  s / r          Toggle staged changes / refresh\n  Ctrl+P or /    Find file (also double-Shift if supported)\n  q / Ctrl+C     Quit\n\nPress any key to close.")
                .block(Block::bordered().title(" Shortcuts ")).style(Style::default().bg(BG).fg(FG)), popup);
        } else if let Some(query) = &self.picker {
            let popup = centered(area, 70, 18);
            frame.render_widget(Clear, popup);
            let block = Block::bordered()
                .title(format!(" Find file: {}▏ ", query))
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

pub fn run(opts: Options, patch: String) -> io::Result<()> {
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
    let mut app = App::new(opts, &patch);
    let mut dirty = true;
    loop {
        if dirty {
            terminal.draw(|frame| app.draw(frame))?;
            dirty = false;
        }
        // First paint always precedes the first syntax request. Neither sending
        // requests nor collecting colors waits for the background worker.
        if let Some(file) = app.files.get(app.selected) {
            app.highlights.request(app.selected, file);
        }
        if app.highlights.receive() {
            dirty = true;
        }
        app.updater.start_check();
        if let Some(message) = app.updater.receive() {
            app.receive_update(message);
            dirty = true;
        }
        if (app.highlights.is_enabled() || app.updater.is_busy() || dirty)
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
    fn update_prompt_requires_explicit_consent_and_defers_while_typing() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut app = App::new(Options::default(), diff::DEMO);
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
        let mut app = App::new(Options::default(), diff::DEMO);
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
        let mut app = App::new(Options::default(), diff::DEMO);
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
            row: app.list_area.y + 1,
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
    fn render_small_and_normal_terminals() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut app = App::new(Options::default(), diff::DEMO);
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
