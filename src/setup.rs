use crate::config::{Config, LocalConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Wrap},
    Frame,
};

const LANGUAGES: [(&str, &str); 3] = [("C#", "c#"), ("C++", "c++"), ("Rust", "rust")];

pub enum Action {
    None,
    Save,
    Skip,
    Quit,
}

pub struct Setup {
    pub target: LocalConfig,
    pub config: Config,
    pub replace: bool,
    pub error: Option<String>,
    selected: usize,
    rows: [Rect; 5],
    save: Rect,
    skip: Rect,
}

impl Setup {
    pub fn new(target: LocalConfig, config: Config, replace: bool) -> Self {
        Self {
            target,
            config,
            replace,
            error: None,
            selected: 0,
            rows: [Rect::default(); 5],
            save: Rect::default(),
            skip: Rect::default(),
        }
    }
    fn toggle(&mut self, index: usize) {
        if index == 4 {
            self.config.ui.icons = !self.config.ui.icons;
        } else if index == 3 {
            self.config.updates.check_on_startup = !self.config.updates.check_on_startup;
        } else {
            let language = LANGUAGES[index].1;
            if self
                .config
                .syntax
                .languages
                .iter()
                .any(|name| name == language)
            {
                self.config.syntax.languages.retain(|name| name != language);
            } else {
                self.config.syntax.languages.push(language.into());
            }
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::None;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(4),
            KeyCode::Tab => self.selected = (self.selected + 1) % 5,
            KeyCode::Char(' ') if key.kind == KeyEventKind::Press => self.toggle(self.selected),
            KeyCode::Char('1'..='5') if key.kind == KeyEventKind::Press => {
                if let KeyCode::Char(c) = key.code {
                    self.toggle(c as usize - '1' as usize);
                }
            }
            KeyCode::Enter | KeyCode::Char('s') if key.kind == KeyEventKind::Press => {
                if !self.save.is_empty() {
                    return Action::Save;
                }
            }
            KeyCode::Esc => return Action::Skip,
            KeyCode::Char('q') => return Action::Quit,
            _ => {}
        }
        Action::None
    }
    pub fn mouse(&mut self, event: MouseEvent) -> Action {
        if event.kind != MouseEventKind::Down(MouseButton::Left) {
            return Action::None;
        }
        let point = (event.column, event.row).into();
        if let Some(index) = self.rows.iter().position(|row| row.contains(point)) {
            self.selected = index;
            self.toggle(index);
        } else if self.save.contains(point) {
            return Action::Save;
        } else if self.skip.contains(point) {
            return Action::Skip;
        }
        Action::None
    }
    pub fn draw(&mut self, frame: &mut Frame) {
        self.rows = [Rect::default(); 5];
        self.save = Rect::default();
        self.skip = Rect::default();
        let area = frame.area();
        let width = area.width.saturating_sub(2).min(78);
        let height = area.height.saturating_sub(2).min(20);
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let block = Block::bordered().title(" Repository setup ").style(
            Style::default()
                .bg(Color::Rgb(19, 23, 31))
                .fg(Color::Rgb(211, 220, 231)),
        );
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        if inner.height < 15 || inner.width < 45 {
            frame.render_widget(
                Paragraph::new("Resize to at least 49 × 19 to configure.\nEsc skips; q quits."),
                inner,
            );
            return;
        }
        let mut lines = vec![
            Line::raw("Choose local preferences for this repository."),
            Line::raw("Syntax starts off. Enable only what you want."),
            Line::raw(""),
        ];
        for (i, (label, language)) in LANGUAGES
            .iter()
            .copied()
            .chain([
                ("Check for updates at startup", ""),
                ("File icons (requires Nerd Font v3+)", ""),
            ])
            .enumerate()
        {
            let checked = if i == 4 {
                self.config.ui.icons
            } else if language.is_empty() {
                self.config.updates.check_on_startup
            } else {
                self.config.syntax.languages.iter().any(|s| s == language)
            };
            self.rows[i] = Rect::new(inner.x, inner.y + 3 + i as u16, inner.width, 1);
            let text = format!(
                "{} [{}] {}  ({})",
                if i == self.selected { "›" } else { " " },
                if checked { "x" } else { " " },
                label,
                i + 1
            );
            lines.push(Line::styled(
                text,
                Style::default().fg(if i == self.selected {
                    Color::Cyan
                } else {
                    Color::White
                }),
            ));
        }
        lines.extend([
            Line::raw(""),
            Line::raw("Save to .tdv.toml at the repository root."),
            Line::raw("Ignored locally through Git's info/exclude."),
            Line::raw("No shared .gitignore change is needed."),
            Line::raw(""),
            Line::from(vec![
                Span::styled("[Enter / s] Save", Style::default().fg(Color::Cyan)),
                Span::raw("   [Esc] Skip   [q] Quit"),
            ]),
            Line::raw("↑↓ / Tab select · Space or 1–5 toggle"),
        ]);
        self.save = Rect::new(inner.x, inner.y + 13, 16, 1);
        self.skip = Rect::new(inner.x + 19, inner.y + 13, 10, 1);
        frame.render_widget(Paragraph::new(lines), inner);
        if let Some(error) = &self.error {
            let area = Rect::new(inner.x, inner.y + 9, inner.width, 3);
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(error.clone())
                    .wrap(Wrap { trim: false })
                    .style(
                        Style::default()
                            .bg(Color::Rgb(19, 23, 31))
                            .fg(Color::LightRed),
                    ),
                area,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::{backend::TestBackend, Terminal};
    fn wizard() -> (tempfile::TempDir, Setup) {
        let root = tempfile::tempdir().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success());
        let setup = Setup::new(
            LocalConfig::discover(root.path()).unwrap(),
            Config::default(),
            false,
        );
        (root, setup)
    }
    #[test]
    fn keyboard_choices_start_off_and_preserve_other_languages() {
        let (_root, mut setup) = wizard();
        assert!(setup.config.syntax.languages.is_empty());
        setup.config.syntax.languages.push("python".into());
        for c in ['1', '3', '4', '5'] {
            setup.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(setup.config.syntax.languages, ["python", "c#", "rust"]);
        assert!(!setup.config.updates.check_on_startup);
        assert!(setup.config.ui.icons);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| setup.draw(frame)).unwrap();
        assert!(matches!(
            setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Action::Save
        ));
        assert!(!setup.target.path.exists()); // Nothing is written until the caller saves.
    }
    #[test]
    fn mouse_controls_and_small_terminal_rendering() {
        let (_root, mut setup) = wizard();
        for (width, height) in [(10, 3), (30, 6), (49, 19), (80, 24)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| setup.draw(frame)).unwrap();
            if width < 49 {
                assert!(matches!(
                    setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                    Action::None
                ));
            }
        }
        let click = |area: Rect| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x,
            row: area.y,
            modifiers: KeyModifiers::NONE,
        };
        setup.mouse(click(setup.rows[1]));
        assert_eq!(setup.config.syntax.languages, ["c++"]);
        assert!(matches!(setup.mouse(click(setup.save)), Action::Save));
        assert!(matches!(setup.mouse(click(setup.skip)), Action::Skip));
        assert!(!setup.target.path.exists());
    }
}
