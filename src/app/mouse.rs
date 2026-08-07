use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::Backend;

use crate::render;

use super::App;

impl<B: Backend> App<B>
where
    B::Error: Send + Sync + 'static,
{
    pub(super) fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {}
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.handle_mouse_scroll(mouse);
                return;
            }
            _ => return,
        }

        // Ignore clicks when the minibuffer is active
        if self.editor.minibuffer.is_active() {
            return;
        }

        self.editor.clear_last_command();

        let click_x = mouse.column;
        let click_y = mouse.row;

        // Calculate pane areas (same logic as update_viewport/render)
        let size = self.terminal.size().unwrap_or_default();
        let comp_height = render::completions_height(&self.editor, size.height, size.width);
        let pane_area = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: size.width,
            height: size.height.saturating_sub(1 + comp_height),
        };

        let (pane_rects, _separators) = self.editor.pane_tree.calculate_rects(pane_area);

        // Find which pane was clicked
        for (path, rect) in &pane_rects {
            // Text area is the pane rect minus the 1-row mode line
            let text_area = ratatui::layout::Rect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height.saturating_sub(1),
            };

            if click_x >= text_area.x
                && click_x < text_area.x + text_area.width
                && click_y >= text_area.y
                && click_y < text_area.y + text_area.height
            {
                // Focus this pane
                self.editor.pane_tree.set_focus_path(path.clone());

                let pane = self.editor.pane_tree.focused_pane();
                let buf = self.editor.buffer_by_id(pane.buffer_id);
                let text_width = text_area.width as usize;

                let rel_x = (click_x - text_area.x) as usize;
                // The clicked screen row plus any visual rows of the top
                // line scrolled off above the viewport gives the visual row
                // counted from the top of the scroll_top line.
                let rel_y = (click_y - text_area.y) as usize
                    + crate::display::clamped_row_offset(pane, buf, text_width);

                let col_in_text = rel_x;

                // Walk buffer lines from scroll_top to find which line the visual row maps to
                let scroll_top = pane.scroll_top;
                let total_lines = buf.line_count();
                let mut visual_row: usize = 0;
                let mut target_line = scroll_top;
                let mut target_row = 0usize;

                let mut line_idx = scroll_top;
                while line_idx < total_lines {
                    let num_visual = crate::display::visual_row_count(buf, line_idx, text_width);

                    if visual_row + num_visual > rel_y {
                        // The click is within this line's visual rows
                        target_line = line_idx;
                        target_row = rel_y - visual_row;
                        break;
                    }

                    visual_row += num_visual;
                    line_idx += 1;
                }

                if line_idx >= total_lines {
                    // Clicked below all content — place at end of buffer
                    let char_count = buf.char_count();
                    self.editor.pane_tree.focused_pane_mut().point = char_count;
                } else {
                    let target_col = crate::display::buffer_col_for_visual_position(
                        buf,
                        target_line,
                        target_row,
                        col_in_text,
                        text_width,
                    );
                    // buffer_col_for_visual_col skips zero-width chars, so
                    // it never lands on a combining mark, but it can land
                    // between a ZWJ and the next emoji of one cluster; snap
                    // out of the cluster.
                    let char_pos = buf
                        .snap_to_grapheme_boundary(buf.line_col_to_char(target_line, target_col));
                    self.editor.pane_tree.focused_pane_mut().point = char_pos;
                }

                self.editor.pane_tree.focused_pane_mut().preferred_column = None;
                return;
            }
        }
    }

    fn handle_mouse_scroll(&mut self, mouse: MouseEvent) {
        let scroll_x = mouse.column;
        let scroll_y = mouse.row;

        let size = self.terminal.size().unwrap_or_default();
        let comp_height = render::completions_height(&self.editor, size.height, size.width);
        let pane_area = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: size.width,
            height: size.height.saturating_sub(1 + comp_height),
        };

        let (pane_rects, _separators) = self.editor.pane_tree.calculate_rects(pane_area);
        // One wheel notch scrolls 3 visual rows, so wrapped lines — even a
        // single line taller than the viewport — scroll through smoothly.
        let scroll_rows: usize = 3;

        for (path, rect) in &pane_rects {
            if scroll_x >= rect.x
                && scroll_x < rect.x + rect.width
                && scroll_y >= rect.y
                && scroll_y < rect.y + rect.height
            {
                let pane = self.editor.pane_tree.pane_at_focus_path(path);
                let buf = self.editor.buffer_by_id(pane.buffer_id);
                let scroll_top = pane.scroll_top;
                let scroll_row_offset = pane.scroll_row_offset;
                let text_width = rect.width as usize;
                let total_lines = buf.line_count();
                let line_len = |l: usize| crate::display::line_visual_width(buf, l);

                let (new_top, new_offset) = match mouse.kind {
                    MouseEventKind::ScrollDown => crate::pane::scroll_down_visual_rows(
                        scroll_top,
                        scroll_row_offset,
                        scroll_rows,
                        total_lines,
                        text_width,
                        line_len,
                    ),
                    MouseEventKind::ScrollUp => crate::pane::scroll_up_visual_rows(
                        scroll_top,
                        scroll_row_offset,
                        scroll_rows,
                        total_lines,
                        text_width,
                        line_len,
                    ),
                    _ => return,
                };

                let pane = self.editor.pane_tree.pane_at_path_pub_mut(path);
                pane.scroll_top = new_top;
                pane.scroll_row_offset = new_offset;
                return;
            }
        }
    }
}
