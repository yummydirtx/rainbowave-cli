use std::io::{self, Write};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    execute,
    style::{force_color_output, Color, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, DisableLineWrap, EnableLineWrap,
        EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    },
};

use crate::render::{Frame, Rgb};

const BEGIN_SYNCHRONIZED_UPDATE: &[u8] = b"\x1b[?2026h";
const END_SYNCHRONIZED_UPDATE: &[u8] = b"\x1b[?2026l";
const CLEAR_SCREEN: &[u8] = b"\x1b[2J";
const UPPER_HALF_BLOCK: &[u8] = "▀".as_bytes();
const MAX_BYTES_PER_CELL: usize = 42;

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
            DisableLineWrap,
            SetBackgroundColor(Color::Black),
            SetForegroundColor(Color::White),
            Clear(ClearType::All),
            MoveTo(0, 0)
        ) {
            let _ = execute!(
                output,
                EndSynchronizedUpdate,
                EnableLineWrap,
                Show,
                ResetColor,
                LeaveAlternateScreen
            );
            let _ = disable_raw_mode();
            return Err(with_context("could not enter alternate screen", error));
        }

        Ok(Self)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        // EndSynchronizedUpdate also recovers cleanly if an interrupted frame left a
        // supporting terminal inside a synchronized update.
        let _ = execute!(
            stdout,
            EndSynchronizedUpdate,
            EnableLineWrap,
            Show,
            ResetColor,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}

/// Serializes a frame into one contiguous write. Modern terminals that implement DEC
/// mode 2026 hold the update until its closing marker, preventing partially painted
/// frames; terminals that do not implement it safely ignore the markers.
pub struct Presenter {
    commands: Vec<u8>,
    dimensions: Option<(u16, u16)>,
}

impl Presenter {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            dimensions: None,
        }
    }

    pub fn invalidate(&mut self) {
        self.dimensions = None;
    }

    pub fn present(&mut self, output: &mut impl Write, frame: &Frame) -> io::Result<()> {
        self.commands.clear();
        let cell_count = usize::from(frame.width()) * usize::from(frame.height());
        let estimated_size = cell_count.saturating_mul(MAX_BYTES_PER_CELL);
        if self.commands.capacity() < estimated_size {
            self.commands.reserve(estimated_size);
        }

        self.commands.extend_from_slice(BEGIN_SYNCHRONIZED_UPDATE);
        let dimensions = (frame.width(), frame.height());
        if self.dimensions != Some(dimensions) {
            self.commands.extend_from_slice(CLEAR_SCREEN);
        }

        let mut active_upper = None;
        let mut active_lower = None;

        for y in 0..frame.height() {
            push_cursor_position(&mut self.commands, y + 1);

            for x in 0..frame.width() {
                let cell = frame.cell(x, y);
                push_colors(
                    &mut self.commands,
                    cell.upper,
                    cell.lower,
                    &mut active_upper,
                    &mut active_lower,
                );
                self.commands.extend_from_slice(UPPER_HALF_BLOCK);
            }
        }

        self.commands.extend_from_slice(END_SYNCHRONIZED_UPDATE);
        output.write_all(&self.commands)?;
        output.flush()?;
        self.dimensions = Some(dimensions);
        Ok(())
    }
}

fn push_cursor_position(output: &mut Vec<u8>, row: u16) {
    output.extend_from_slice(b"\x1b[");
    push_decimal(output, row);
    output.extend_from_slice(b";1H");
}

fn push_colors(
    output: &mut Vec<u8>,
    upper: Rgb,
    lower: Rgb,
    active_upper: &mut Option<Rgb>,
    active_lower: &mut Option<Rgb>,
) {
    let upper_changed = *active_upper != Some(upper);
    let lower_changed = *active_lower != Some(lower);

    if !upper_changed && !lower_changed {
        return;
    }

    output.extend_from_slice(b"\x1b[");
    if upper_changed {
        output.extend_from_slice(b"38;2;");
        push_rgb(output, upper);
    }
    if upper_changed && lower_changed {
        output.push(b';');
    }
    if lower_changed {
        output.extend_from_slice(b"48;2;");
        push_rgb(output, lower);
    }
    output.push(b'm');

    *active_upper = Some(upper);
    *active_lower = Some(lower);
}

fn push_rgb(output: &mut Vec<u8>, color: Rgb) {
    push_decimal(output, u16::from(color.red));
    output.push(b';');
    push_decimal(output, u16::from(color.green));
    output.push(b';');
    push_decimal(output, u16::from(color.blue));
}

fn push_decimal(output: &mut Vec<u8>, mut value: u16) {
    let mut digits = [0_u8; 5];
    let mut cursor = digits.len();

    loop {
        cursor -= 1;
        digits[cursor] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }

    output.extend_from_slice(&digits[cursor..]);
}

fn with_context(context: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{push_decimal, Presenter, BEGIN_SYNCHRONIZED_UPDATE, END_SYNCHRONIZED_UPDATE};
    use crate::render::Frame;

    #[test]
    fn decimal_encoder_handles_full_u16_range() {
        for value in [0, 7, 42, 255, 1_024, u16::MAX] {
            let mut encoded = Vec::new();
            push_decimal(&mut encoded, value);
            assert_eq!(String::from_utf8(encoded).unwrap(), value.to_string());
        }
    }

    #[test]
    fn frame_is_batched_inside_synchronized_update_markers() {
        let frame = Frame::new(3, 2);
        let mut presenter = Presenter::new();
        let mut output = Vec::new();
        presenter.present(&mut output, &frame).unwrap();

        assert!(output.starts_with(BEGIN_SYNCHRONIZED_UPDATE));
        assert!(output.ends_with(END_SYNCHRONIZED_UPDATE));
        assert_eq!(
            output
                .windows("▀".len())
                .filter(|bytes| *bytes == "▀".as_bytes())
                .count(),
            6
        );
        assert!(output.windows(4).any(|bytes| bytes == b"\x1b[2J"));
    }

    #[test]
    fn unchanged_dimensions_do_not_clear_the_screen_again() {
        let frame = Frame::new(2, 1);
        let mut presenter = Presenter::new();
        presenter.present(&mut Vec::new(), &frame).unwrap();

        let mut second_output = Vec::new();
        presenter.present(&mut second_output, &frame).unwrap();
        assert!(!second_output.windows(4).any(|bytes| bytes == b"\x1b[2J"));

        presenter.invalidate();
        let mut invalidated_output = Vec::new();
        presenter.present(&mut invalidated_output, &frame).unwrap();
        assert!(invalidated_output
            .windows(4)
            .any(|bytes| bytes == b"\x1b[2J"));
    }
}
