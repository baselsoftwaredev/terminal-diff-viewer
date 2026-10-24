//! All syntax loading and parsing happens on one background worker.
use crate::diff::{FileDiff, Kind};
use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc,
    },
    thread,
};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, ThemeSet},
    parsing::SyntaxSet,
};

// Bound retained work and memory. Oversized/unsupported files keep diff colors.
const MAX_BYTES: usize = 512 * 1024;
const MAX_LINES: usize = 10_000;
const MAX_LINE_BYTES: usize = 8_192;
const MAX_TOKENS: usize = 50_000;
const CACHE_FILES: usize = 4;

pub type Tokens = Vec<Span<'static>>;
#[derive(Default)]
pub struct Colors {
    pub lines: Vec<[Option<Tokens>; 2]>,
}
struct Request {
    epoch: u64,
    file: usize,
    source: Arc<FileDiff>,
}
struct Reply {
    epoch: u64,
    file: usize,
    colors: Arc<Colors>,
}

pub struct Highlighter {
    requests: Option<SyncSender<Request>>,
    replies: Receiver<Reply>,
    epoch: Arc<AtomicU64>,
    requested: Option<usize>,
    cache: VecDeque<(usize, Arc<Colors>)>,
}

impl Highlighter {
    pub fn new(languages: Vec<String>) -> Self {
        let (tx, rx) = mpsc::sync_channel::<Request>(1);
        let (reply_tx, replies) = mpsc::channel();
        let epoch = Arc::new(AtomicU64::new(0));
        if languages.is_empty() {
            // Off means no worker thread and no syntax/theme initialization.
            return Self {
                requests: None,
                replies,
                epoch,
                requested: None,
                cache: VecDeque::new(),
            };
        }
        let current = Arc::clone(&epoch);
        // Starting the worker must not turn a resource shortage into a failed viewer.
        let _ = thread::Builder::new()
            .name("syntax-highlighter".into())
            .spawn(move || {
                // Lazy initialization: the worker waits for the first painted file.
                let mut engine = None;
                while let Ok(request) = rx.recv() {
                    if current.load(Ordering::Relaxed) != request.epoch {
                        continue;
                    }
                    let (syntaxes, themes) = engine.get_or_insert_with(|| {
                        (
                            SyntaxSet::load_defaults_newlines(),
                            ThemeSet::load_defaults(),
                        )
                    });
                    if let Some(colors) =
                        colorize(&request.source, syntaxes, themes, &languages, || {
                            current.load(Ordering::Relaxed) != request.epoch
                        })
                    {
                        if reply_tx
                            .send(Reply {
                                epoch: request.epoch,
                                file: request.file,
                                colors: Arc::new(colors),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            });
        Self {
            requests: Some(tx),
            replies,
            epoch,
            requested: None,
            cache: VecDeque::new(),
        }
    }

    pub fn selection_changed(&mut self) {
        self.epoch.fetch_add(1, Ordering::Relaxed);
        self.requested = None;
    }

    pub fn is_enabled(&self) -> bool {
        self.requests.is_some()
    }

    pub fn invalidate(&mut self) {
        self.selection_changed();
        self.cache.clear();
    }

    /// Called after drawing. Sharing an Arc never copies the patch on the UI thread.
    pub fn request(&mut self, file: usize, source: &Arc<FileDiff>) {
        let Some(requests) = &self.requests else {
            return;
        };
        if self.requested == Some(file) || self.get(file).is_some() {
            return;
        }
        let request = Request {
            epoch: self.epoch.load(Ordering::Relaxed),
            file,
            source: Arc::clone(source),
        };
        match requests.try_send(request) {
            Ok(()) | Err(mpsc::TrySendError::Disconnected(_)) => self.requested = Some(file),
            Err(mpsc::TrySendError::Full(_)) => {} // Retry later without waiting.
        }
    }

    pub fn receive(&mut self) -> bool {
        let mut changed = false;
        while let Ok(reply) = self.replies.try_recv() {
            if reply.epoch != self.epoch.load(Ordering::Relaxed) {
                continue;
            }
            self.cache.retain(|(file, _)| *file != reply.file);
            if self.cache.len() == CACHE_FILES {
                self.cache.pop_front();
            }
            self.cache.push_back((reply.file, reply.colors));
            changed = true;
        }
        changed
    }

    pub fn get(&self, file: usize) -> Option<&Colors> {
        self.cache
            .iter()
            .find(|(f, _)| *f == file)
            .map(|(_, c)| c.as_ref())
    }
}

impl Drop for Highlighter {
    fn drop(&mut self) {
        // Never join: exiting the viewer must not wait for a regex to finish.
        self.epoch.fetch_add(1, Ordering::Relaxed);
    }
}

fn colorize(
    file: &FileDiff,
    syntaxes: &SyntaxSet,
    themes: &ThemeSet,
    languages: &[String],
    cancelled: impl Fn() -> bool,
) -> Option<Colors> {
    if cancelled() {
        return None;
    }
    if file.lines.len() > MAX_LINES
        || file.lines.iter().any(|l| l.text.len() > MAX_LINE_BYTES)
        || file.lines.iter().map(|l| l.text.len()).sum::<usize>() > MAX_BYTES
    {
        return Some(Colors::default());
    }
    let path = file.name.trim_matches('"').trim_end_matches('\t');
    let syntax = Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(|ext| syntaxes.find_syntax_by_extension(ext))
        .or_else(|| {
            Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| syntaxes.find_syntax_by_extension(name))
        });
    let Some(syntax) = syntax else {
        return Some(Colors::default());
    };
    if !languages
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&syntax.name))
    {
        return Some(Colors::default());
    }
    let theme = &themes.themes["base16-ocean.dark"];
    let mut old = HighlightLines::new(syntax, theme);
    let mut new = HighlightLines::new(syntax, theme);
    let mut result = Colors {
        lines: Vec::with_capacity(file.lines.len()),
    };
    let mut token_count = 0;
    for line in &file.lines {
        if cancelled() {
            return None;
        }
        let mut pair = [None, None];
        if line.kind == Kind::Header {
            // Missing context between hunks must not leak multiline state.
            if line.text.starts_with("@@ ") {
                old = HighlightLines::new(syntax, theme);
                new = HighlightLines::new(syntax, theme);
            }
        } else {
            let text = format!("{}\n", line.text);
            for (side, engine) in [&mut old, &mut new].into_iter().enumerate() {
                if (side == 0 && line.kind == Kind::Added)
                    || (side == 1 && line.kind == Kind::Removed)
                {
                    continue;
                }
                if let Ok(ranges) = engine.highlight_line(&text, syntaxes) {
                    token_count += ranges.len();
                    if token_count > MAX_TOKENS {
                        return Some(Colors::default());
                    }
                    pair[side] = Some(
                        ranges
                            .into_iter()
                            .filter_map(|(style, text)| {
                                let text = text.trim_end_matches('\n');
                                if text.is_empty() {
                                    return None;
                                }
                                let c = style.foreground;
                                let mut s = Style::default().fg(Color::Rgb(c.r, c.g, c.b));
                                if style.font_style.contains(FontStyle::BOLD) {
                                    s = s.add_modifier(Modifier::BOLD);
                                }
                                if style.font_style.contains(FontStyle::ITALIC) {
                                    s = s.add_modifier(Modifier::ITALIC);
                                }
                                Some(Span::styled(text.to_owned(), s))
                            })
                            .collect(),
                    );
                }
            }
        }
        result.lines.push(pair);
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_off_creates_no_worker_or_requests() {
        let mut worker = Highlighter::new(Vec::new());
        let source = Arc::new(crate::diff::parse(crate::diff::DEMO).remove(0));
        worker.request(0, &source);
        assert!(!worker.is_enabled());
        assert!(worker.requests.is_none());
        assert!(worker.requested.is_none());
        assert!(!worker.receive());
    }
    #[test]
    fn only_opted_in_languages_are_highlighted() {
        let syntaxes = SyntaxSet::load_defaults_newlines();
        let themes = ThemeSet::load_defaults();
        let mut file = crate::diff::parse(crate::diff::DEMO).remove(0);
        let languages: Vec<String> = ["csharp", "cpp", "rust"]
            .iter()
            .map(|s| crate::config::language_name(s))
            .collect();
        for name in ["code.rs", "code.cs", "code.csx", "code.cpp", "code.hpp"] {
            file.name = name.into();
            assert!(
                !colorize(&file, &syntaxes, &themes, &languages, || false)
                    .unwrap()
                    .lines
                    .is_empty(),
                "{name}"
            );
        }
        for name in ["code.py", "code.rb", "code.go", "code.js", "code.md"] {
            file.name = name.into();
            assert!(
                colorize(&file, &syntaxes, &themes, &languages, || false)
                    .unwrap()
                    .lines
                    .is_empty(),
                "{name}"
            );
        }
    }
    #[test]
    fn syntax_preserves_text_and_distinguishes_old_and_new() {
        let files = crate::diff::parse(crate::diff::DEMO);
        let syntaxes = SyntaxSet::load_defaults_newlines();
        let themes = ThemeSet::load_defaults();
        let languages = vec!["rust".into()];
        let colors = colorize(&files[0], &syntaxes, &themes, &languages, || false).unwrap();
        for (line, pair) in files[0].lines.iter().zip(&colors.lines) {
            for tokens in pair.iter().flatten() {
                assert_eq!(
                    tokens
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>(),
                    line.text
                );
            }
            if line.kind == Kind::Added {
                assert!(pair[0].is_none());
                assert!(pair[1].is_some());
            }
            if line.kind == Kind::Removed {
                assert!(pair[0].is_some());
                assert!(pair[1].is_none());
            }
        }
        let tokens = colors
            .lines
            .iter()
            .flat_map(|p| p.iter().flatten())
            .flatten();
        let distinct: std::collections::HashSet<_> = tokens.map(|t| t.style.fg).collect();
        assert!(distinct.len() > 1);
        assert!(colorize(&files[0], &syntaxes, &themes, &languages, || true).is_none());
    }
    #[test]
    fn busy_worker_does_not_block_requests_or_result_checks() {
        let (_tx, replies) = mpsc::channel();
        let (requests, rx) = mpsc::sync_channel(1);
        let mut worker = Highlighter {
            requests: Some(requests),
            replies,
            epoch: Arc::new(AtomicU64::new(0)),
            requested: None,
            cache: VecDeque::new(),
        };
        let source = Arc::new(crate::diff::parse(crate::diff::DEMO).remove(0));
        worker.request(0, &source);
        worker.selection_changed();
        // No worker consumes the full queue. This must return without waiting.
        worker.request(1, &source);
        assert!(worker.requested.is_none());
        assert!(!worker.receive());
        assert!(worker.get(1).is_none());
        assert_eq!(rx.try_recv().unwrap().file, 0);
        worker.request(1, &source);
        assert_eq!(rx.try_recv().unwrap().file, 1);
    }
    #[test]
    fn unknown_and_oversized_sources_keep_plain_colors() {
        let syntaxes = SyntaxSet::load_defaults_newlines();
        let themes = ThemeSet::load_defaults();
        let mut file = crate::diff::parse(crate::diff::DEMO).remove(0);
        file.name = "file.unknown-language".into();
        assert!(
            colorize(&file, &syntaxes, &themes, &["rust".into()], || false)
                .unwrap()
                .lines
                .is_empty()
        );
        file.name = "file.rs".into();
        file.lines[0].text = "x".repeat(MAX_LINE_BYTES + 1);
        assert!(
            colorize(&file, &syntaxes, &themes, &["rust".into()], || false)
                .unwrap()
                .lines
                .is_empty()
        );
    }
    #[test]
    fn stale_results_cannot_recolor_refreshed_files() {
        let (tx, replies) = mpsc::channel();
        let (requests, _rx) = mpsc::sync_channel(1);
        let mut worker = Highlighter {
            requests: Some(requests),
            replies,
            epoch: Arc::new(AtomicU64::new(0)),
            requested: None,
            cache: VecDeque::new(),
        };
        tx.send(Reply {
            epoch: 0,
            file: 0,
            colors: Arc::new(Colors::default()),
        })
        .unwrap();
        worker.invalidate();
        assert!(!worker.receive());
        assert!(worker.get(0).is_none());
        tx.send(Reply {
            epoch: 1,
            file: 0,
            colors: Arc::new(Colors::default()),
        })
        .unwrap();
        assert!(worker.receive());
        assert!(worker.get(0).is_some());
    }
}
