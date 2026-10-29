//! Terminal history selection with a wrapped, scrollable transcript preview.

use super::{HistoryEntry, HistoryReference, IpcCommand, IpcResponse};
use anyhow::{bail, Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

struct Picker {
    entries: Vec<HistoryEntry>,
    selected: usize,
    scroll: u16,
    page_height: u16,
    max_scroll: u16,
    text: String,
    error: Option<String>,
}

impl Picker {
    /// Load the selected entry's complete text and reset preview scrolling.
    fn load(&mut self, fetch: &mut impl FnMut(&HistoryReference) -> Result<String>) {
        self.scroll = 0;
        self.error = None;
        match fetch(&self.entries[self.selected].reference) {
            Ok(text) => self.text = text,
            Err(error) => {
                self.text.clear();
                self.error = Some(format!("{error:#}"));
            }
        }
    }

    /// Handle navigation and return whether copying or cancellation ended the picker.
    fn key(
        &mut self,
        key: KeyEvent,
        fetch: &mut impl FnMut(&HistoryReference) -> Result<String>,
        copy: &mut impl FnMut(&HistoryReference) -> Result<()>,
    ) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        match key.code {
            KeyCode::Esc => return true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return true,
            KeyCode::Up if self.selected > 0 => {
                self.selected -= 1;
                self.load(fetch);
            }
            KeyCode::Down if self.selected + 1 < self.entries.len() => {
                self.selected += 1;
                self.load(fetch);
            }
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(self.page_height),
            KeyCode::PageDown => {
                self.scroll = self
                    .scroll
                    .saturating_add(self.page_height)
                    .min(self.max_scroll)
            }
            KeyCode::Enter => match copy(&self.entries[self.selected].reference) {
                Ok(()) => return true,
                Err(error) => self.error = Some(format!("{error:#}")),
            },
            _ => {}
        }
        false
    }

    /// Render the selected transcript and clamp its scroll to the resized viewport.
    fn draw(&mut self, frame: &mut Frame<'_>) {
        let areas = Layout::vertical([
            Constraint::Length(
                (self.entries.len() as u16 + 2)
                    .min(frame.area().height / 3)
                    .max(3),
            ),
            Constraint::Min(1),
            Constraint::Length(if self.error.is_some() { 3 } else { 1 }),
        ])
        .split(frame.area());
        let items = self.entries.iter().map(|entry| {
            ListItem::new(format!(
                "{}  {}  {} chars  {}",
                entry.index,
                super::client::format_age(entry.age_secs),
                entry.chars,
                entry.preview,
            ))
        });
        let list = List::new(items)
            .block(
                Block::default()
                    .title("Transcript history")
                    .borders(Borders::ALL),
            )
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        let mut selection = ListState::default().with_selected(Some(self.selected));
        frame.render_stateful_widget(list, areas[0], &mut selection);
        let preview_block = Block::default()
            .title("Full transcript")
            .borders(Borders::ALL);
        let preview_area = preview_block.inner(areas[1]);
        let preview = Paragraph::new(self.text.as_str()).wrap(Wrap { trim: false });
        self.page_height = preview_area.height.max(1);
        self.max_scroll = u16::try_from(
            preview
                .line_count(preview_area.width)
                .saturating_sub(preview_area.height as usize),
        )
        .unwrap_or(u16::MAX);
        self.scroll = self.scroll.min(self.max_scroll);
        frame.render_widget(
            preview.block(preview_block).scroll((self.scroll, 0)),
            areas[1],
        );
        let footer = match &self.error {
            Some(error) => {
                format!("{error}\nUp/Down select  PageUp/PageDown scroll  Enter copy  Esc cancel")
            }
            None => "Up/Down select  PageUp/PageDown scroll  Enter copy  Esc cancel".to_owned(),
        };
        frame.render_widget(Paragraph::new(footer).wrap(Wrap { trim: false }), areas[2]);
    }
}

/// Browse a nonempty history snapshot and copy its selected entry through IPC.
///
/// # Returns
///
/// Success after copying or cancellation, with the terminal restored.
///
/// # Errors
///
/// Returns terminal errors. Per-entry lookup and clipboard errors remain visible
/// in the picker so the user can select another entry or cancel.
pub(super) fn run(entries: Vec<HistoryEntry>) -> Result<()> {
    let mut fetch =
        |reference: &HistoryReference| match super::send_command(&IpcCommand::HistoryText {
            reference: reference.clone(),
        })? {
            IpcResponse::HistoryText { text } => Ok(text),
            IpcResponse::Err { message } => bail!(message),
            _ => bail!("unexpected transcript response"),
        };
    let mut copy =
        |reference: &HistoryReference| match super::send_command(&IpcCommand::CopyHistory {
            reference: reference.clone(),
        })? {
            IpcResponse::Ok { .. } => Ok(()),
            IpcResponse::Err { message } => bail!(message),
            _ => bail!("unexpected clipboard response"),
        };
    let mut picker = Picker {
        entries,
        selected: 0,
        scroll: 0,
        page_height: 1,
        max_scroll: 0,
        text: String::new(),
        error: None,
    };
    picker.load(&mut fetch);
    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = ratatui::try_restore();
            return Err(error).context("initialize history picker terminal");
        }
    };
    with_restoration(
        || loop {
            terminal.draw(|frame| picker.draw(frame))?;
            if let Event::Key(key) = event::read()? {
                if picker.key(key, &mut fetch, &mut copy) {
                    return Ok(());
                }
            }
        },
        ratatui::try_restore,
    )
}

/// Restore terminal modes after either a successful run or a returned error.
fn with_restoration<T>(
    run: impl FnOnce() -> Result<T>,
    restore: impl FnOnce() -> std::io::Result<()>,
) -> Result<T> {
    let result = run();
    let restored = restore().context("restore terminal after history picker");
    match result {
        Ok(value) => {
            restored?;
            Ok(value)
        }
        Err(error) => {
            let _ = restored;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker() -> Picker {
        let state = super::super::SharedState::new();
        state.remember_transcript("first\n雪".into());
        state.remember_transcript("second".into());
        Picker {
            entries: state.history_snapshot(None),
            selected: 0,
            scroll: 0,
            page_height: 3,
            max_scroll: 9,
            text: "second".into(),
            error: None,
        }
    }

    #[test]
    fn arrows_fetch_original_reference_and_copy_failure_keeps_picker_open() {
        let mut picker = picker();
        let selected = picker.entries[1].reference.clone();
        let mut fetch = |reference: &HistoryReference| {
            assert_eq!(reference, &selected);
            Ok("first\n雪".into())
        };
        assert!(
            !picker.key(KeyCode::Down.into(), &mut fetch, &mut |_| bail!(
                "unavailable"
            ))
        );
        assert_eq!(picker.text, "first\n雪");
        assert!(
            !picker.key(KeyCode::Enter.into(), &mut fetch, &mut |reference| {
                assert_eq!(reference, &selected);
                bail!("clipboard unavailable")
            })
        );
        assert_eq!(picker.error.as_deref(), Some("clipboard unavailable"));
        assert!(picker.key(KeyCode::Enter.into(), &mut fetch, &mut |_| Ok(())));
    }

    #[test]
    fn cancellation_does_not_copy_and_pages_are_bounded() {
        let mut picker = picker();
        let mut fetch = |_: &HistoryReference| panic!("unexpected fetch");
        let mut copy = |_: &HistoryReference| panic!("unexpected copy");
        picker.key(KeyCode::PageDown.into(), &mut fetch, &mut copy);
        assert_eq!(picker.scroll, 3);
        picker.key(KeyCode::PageUp.into(), &mut fetch, &mut copy);
        assert_eq!(picker.scroll, 0);
        assert!(picker.key(KeyCode::Esc.into(), &mut fetch, &mut copy));
        assert!(picker.key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            &mut fetch,
            &mut copy
        ));
    }

    #[test]
    fn restoration_runs_on_success_and_terminal_error() {
        for fails in [false, true] {
            let restored = std::cell::Cell::new(false);
            let result = with_restoration(
                || {
                    if fails {
                        bail!("terminal failed")
                    } else {
                        Ok(())
                    }
                },
                || {
                    restored.set(true);
                    Ok(())
                },
            );
            assert_eq!(result.is_err(), fails);
            assert!(restored.get());
        }
    }

    #[test]
    fn preview_wraps_and_survives_small_resized_terminals() {
        let mut picker = picker();
        picker.text = "雪 wrapped text\n".repeat(100);
        for (width, height) in [(80, 24), (25, 10), (1, 1)] {
            let backend = ratatui::backend::TestBackend::new(width, height);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal.draw(|frame| picker.draw(frame)).unwrap();
            if width == 80 {
                let screen: String = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(screen.contains('雪'));
                assert!(screen.contains("Full transcript"));
                assert!(picker.max_scroll > 0);
            }
        }
    }
}
