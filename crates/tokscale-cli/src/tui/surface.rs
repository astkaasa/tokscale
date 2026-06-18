use anyhow::Result;
use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};

use super::{ui, App};

pub(crate) fn render_app_buffer(app: &mut App, width: u16, height: u16) -> Result<Buffer> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|frame| ui::render(frame, app))?;
    Ok(terminal.backend().buffer().clone())
}
