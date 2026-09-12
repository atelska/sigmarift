use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    widgets::{Block, Clear},
};

use super::theme;

/// Centers and paints the opaque support surface shared by every modal.
///
/// Width is specified in terminal cells so compact menus do not expand with a
/// wide terminal. It is clipped only when the terminal itself is narrower.
pub fn overlay(frame: &mut Frame, width: u16, height: u16) -> Rect {
    let area = frame.area();
    let width = width.min(area.width.saturating_sub(2)).max(1);
    let [_, horizontal, _] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(width),
        Constraint::Fill(1),
    ])
    .areas(area);
    let [_, vertical, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height.min(area.height)),
        Constraint::Fill(1),
    ])
    .areas(horizontal);

    frame.render_widget(Clear, vertical);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::RAISED_BACKGROUND)),
        vertical,
    );
    vertical
}
