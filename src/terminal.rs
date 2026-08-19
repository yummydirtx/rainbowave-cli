use std::io::{self, Write};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    execute, queue,
    style::{force_color_output, Color, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};

use crate::render::{Frame, Rgb};

pub struct Session;

impl Session {
    pub fn enter(output: &mut impl Write) -> io::Result<Self> {
        // Color is the program's output rather than optional decoration, so render it even
        // when a parent process exports NO_COLOR.
        force_color_output(true);
        enable_raw_mode()
            .map_err(|error| with_context("could not enable raw terminal mode", error))?;

        if let Err(error) = execute!(
            output,
            EnterAlternateScreen,
            Hide,
            SetBackgroundColor(Color::Black),
            SetForegroundColor(Color::White),
            Clear(ClearType::All),
            MoveTo(0, 0)
        ) {
            let _ = execute!(output, Show, ResetColor, LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(with_context("could not enter alternate screen", error));
        }

        Ok(Self)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = execute!(stdout, Show, ResetColor, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

pub fn present(output: &mut impl Write, frame: &Frame, previous: Option<&Frame>) -> io::Result<()> {
    let same_dimensions =
        previous.is_some_and(|old| old.width() == frame.width() && old.height() == frame.height());

    if !same_dimensions {
        queue!(output, Clear(ClearType::All))?;
    }

    let mut active_color = None;

    for y in 0..frame.height() {
        for x in 0..frame.width() {
            let cell = frame.cell(x, y);
            let old_cell = if same_dimensions {
                previous.and_then(|old| old.cell(x, y))
            } else {
                None
            };

            if cell == old_cell {
                continue;
            }

            queue!(output, MoveTo(x, y))?;
            match cell {
                Some(cell) => {
                    if active_color != Some(cell.color) {
                        queue!(output, SetForegroundColor(to_terminal_color(cell.color)))?;
                        active_color = Some(cell.color);
                    }
                    write!(output, "{}", cell.glyph)?;
                }
                None => write!(output, " ")?,
            }
        }
    }

    output.flush()
}

fn to_terminal_color(color: Rgb) -> Color {
    Color::Rgb {
        r: color.red,
        g: color.green,
        b: color.blue,
    }
}

fn with_context(context: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{context}: {error}"))
}
