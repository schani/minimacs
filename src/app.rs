use anyhow::Result;
use crossterm::event::{Event, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::backend::Backend;
use ratatui::Terminal;

use crate::editor::Editor;
use crate::event::{EventSource, Poll};
use crate::render;
use crate::syntax_worker::SyntaxWorker;

mod input;
mod mouse;

use input::InputState;

pub struct App<B: Backend> {
    editor: Editor,
    terminal: Terminal<B>,
    input: InputState,
    syntax_worker: SyntaxWorker,
    /// Number of `render()` calls, so tests can assert that discarded
    /// events (mouse motion, key releases, focus changes) skip the redraw.
    #[cfg(test)]
    renders: usize,
}

impl<B: Backend> App<B>
where
    B::Error: Send + Sync + 'static,
{
    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    pub fn new(terminal: Terminal<B>, editor: Editor) -> Self {
        Self {
            editor,
            terminal,
            input: InputState::new(),
            syntax_worker: SyntaxWorker::new(),
            #[cfg(test)]
            renders: 0,
        }
    }

    pub fn run(&mut self, event_source: &mut dyn EventSource) -> Result<()> {
        self.update_viewport();
        self.render()?;

        loop {
            let event = match event_source.next_event() {
                Poll::Event(event) => event,
                // Timeouts deliver no event; nothing can have changed, so
                // skip the re-render instead of redrawing ~10×/s while idle.
                Poll::Timeout => {
                    if self.apply_syntax_completions() {
                        self.render()?;
                    }
                    continue;
                }
                // The terminal is gone (tty hangup): no further input can
                // arrive, so exit instead of spinning on a dead source.
                // We can't prompt about unsaved buffers — there is no input
                // to answer with — so the editor just quits; main still
                // restores the terminal on this error path.
                Poll::Closed => anyhow::bail!("event source closed"),
            };
            let state_changed = self.dispatch_event(event);
            let syntax_changed = self.apply_syntax_completions();
            if self.editor.should_quit() {
                break;
            }

            // Discarded events (bare mouse motion, key releases, focus
            // changes) change nothing, so skip the redraw. Any-motion mouse
            // tracking (mode 1003) floods `Moved` events on bare movement;
            // rendering each one is a render storm.
            if state_changed || syntax_changed {
                self.update_viewport();
                self.render()?;
            }
        }
        Ok(())
    }

    /// Run until all events are consumed (for tests). Mirrors `run()`'s
    /// render gating so tests can assert which events cause a redraw.
    #[cfg(test)]
    pub fn run_until_idle(&mut self, event_source: &mut dyn EventSource) -> Result<()> {
        self.update_viewport();
        self.render()?;

        while let Poll::Event(event) = event_source.next_event() {
            let state_changed = self.dispatch_event(event);
            if self.editor.should_quit() {
                break;
            }
            if state_changed {
                self.update_viewport();
                self.render()?;
            }
        }
        Ok(())
    }

    /// Route one input event to its handler. This is the single place that
    /// decides, per event kind, what happens to the pending input state
    /// (`InputState`): key events consume or reset it inside `handle_key`;
    /// paste and acted-on mouse events (left click, scroll) cancel any
    /// pending chord and pending ESC before being handled
    /// (cancel-then-handle, so a click mid-chord both cancels the chord and
    /// performs the click); discarded mouse events (bare motion, drags,
    /// button releases, non-left buttons) touch nothing — merely moving the
    /// mouse over the terminal must not cancel a chord — and a resize
    /// intentionally leaves a chord in progress alone.
    ///
    /// Returns whether the event may have changed visible state; `run()`
    /// skips the redraw when it did not. Key presses conservatively report
    /// true — whether a command actually changed anything is the editor's
    /// business, and over-rendering a keystroke is cheap.
    fn dispatch_event(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key_event) => {
                // Act only on Press and Repeat (a held key must still
                // repeat). Windows and kitty-protocol terminals also report
                // Release events; letting those through would execute every
                // keystroke twice.
                if key_event.kind == KeyEventKind::Release {
                    return false;
                }
                self.handle_key(key_event);
                true
            }
            Event::Paste(text) => {
                self.input.reset();
                // Paste during isearch extends the query (isearch-yank)
                // instead of inserting into a buffer.
                if self.editor.isearch().is_some() {
                    self.editor.isearch_yank(&text);
                } else {
                    self.editor.paste_supplied_text(&text);
                }
                true
            }
            Event::Mouse(mouse_event) => match mouse_event.kind {
                MouseEventKind::Down(MouseButton::Left)
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollDown => {
                    self.input.reset();
                    self.handle_mouse(mouse_event);
                    true
                }
                // Everything else is discarded without touching any state:
                // any-motion tracking (mode 1003) reports every bare mouse
                // movement, so motion must neither cancel a pending chord
                // nor trigger a render.
                _ => false,
            },
            // ratatui needs a redraw to re-layout after a resize.
            Event::Resize(_, _) => true,
            // FocusGained/FocusLost: no handler, nothing changed.
            _ => false,
        }
    }

    fn update_viewport(&mut self) {
        let size = self.terminal.size().unwrap_or_default();
        let layout = render::screen_layout(
            &self.editor,
            ratatui::layout::Rect::new(0, 0, size.width, size.height),
        );

        let (pane_rects, _separators) = self.editor.pane_tree().calculate_rects(layout.pane_area);
        let mut dimensions_changed = false;
        for (path, rect) in &pane_rects {
            // Each pane rect includes 1 row for mode line
            let text_height = rect.height.saturating_sub(1) as usize;
            let text_width = rect.width as usize;
            let pane = self.editor.pane_tree().pane_at_focus_path(path);
            dimensions_changed |=
                pane.viewport_height() != text_height || pane.viewport_width() != text_width;
            self.editor
                .update_pane_viewport(path, text_height, text_width);
        }

        let minibuffer_width = layout.minibuffer_area.width as usize;
        let minibuffer_height = layout.minibuffer_area.height as usize;
        dimensions_changed |= self.editor.minibuffer_pane().viewport_width() != minibuffer_width
            || self.editor.minibuffer_pane().viewport_height() != minibuffer_height;
        self.editor
            .update_minibuffer_viewport(minibuffer_height, minibuffer_width);

        // Reflow can move point below the viewport even though no editing
        // command ran. Apply every new dimension first, then reveal the
        // focused cursor. Unchanged dimensions do not undo intentional mouse
        // scrolling that leaves point off-screen.
        if dimensions_changed {
            self.editor.ensure_cursor_visible();
        }
    }

    pub fn render(&mut self) -> Result<()> {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        let editor = &self.editor;
        let pending = self.input.render_view();
        let pending_input = render::PendingInput { display: &pending };
        self.terminal.draw(|frame| {
            render::render(frame, editor, &self.syntax_worker, pending_input);
        })?;
        Ok(())
    }

    fn apply_syntax_completions(&mut self) -> bool {
        let mut changed = false;
        for completion in self.syntax_worker.take_completions() {
            let (accepted, show_disabled_message) =
                self.editor.accept_syntax_completion(completion);
            if show_disabled_message {
                self.editor.set_pending_display_message(
                    "Syntax highlighting disabled (parse timeout)".to_string(),
                );
            }
            changed |= accepted;
        }
        changed
    }
}

#[cfg(test)]
mod tests;
