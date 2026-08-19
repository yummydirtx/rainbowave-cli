use std::{f32::consts::TAU, time::Duration};

const LAYER_COUNT: usize = 5;
const WAVE_CYCLES: f32 = 1.55;
const WAVE_SPEED: f32 = 1.7;
const COLOR_SPEED: f32 = 0.12;
const LAYER_GLYPHS: [char; LAYER_COUNT] = ['~', '=', '-', '~', '*'];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cell {
    pub glyph: char,
    pub color: Rgb,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    width: u16,
    height: u16,
    cells: Vec<Option<Cell>>,
}

impl Frame {
    pub fn at_time(width: u16, height: u16, elapsed: Duration) -> Self {
        let mut frame = Self {
            width,
            height,
            cells: vec![None; usize::from(width) * usize::from(height)],
        };

        if width == 0 || height == 0 {
            return frame;
        }

        let time = elapsed.as_secs_f32();
        let amplitude = (f32::from(height) * 0.22).max(1.0);
        let center = (f32::from(height) - 1.0) / 2.0;
        let layer_spacing = f32::from(height) * 0.055;
        let thickness = if height >= 18 { 2 } else { 1 };

        for (layer, &glyph) in LAYER_GLYPHS.iter().enumerate() {
            let layer_offset = layer as f32 - (LAYER_COUNT - 1) as f32 / 2.0;
            let layer_phase = layer as f32 * 0.68;
            let layer_center = center + layer_offset * layer_spacing;

            for x in 0..width {
                let horizontal_position = f32::from(x) / f32::from(width.max(1));
                let angle =
                    horizontal_position * TAU * WAVE_CYCLES + time * WAVE_SPEED + layer_phase;
                let wave_height = layer_center + angle.sin() * amplitude;
                let hue = (horizontal_position + time * COLOR_SPEED + layer as f32 * 0.045)
                    .rem_euclid(1.0);
                let color = hsv_to_rgb(hue, 0.88, 1.0);

                for stroke in 0..thickness {
                    let y = wave_height.round() as i32 + stroke;
                    if (0..i32::from(height)).contains(&y) {
                        frame.set(x, y as u16, Cell { glyph, color });
                    }
                }
            }
        }

        frame
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn cell(&self, x: u16, y: u16) -> Option<Cell> {
        self.cells[self.index(x, y)]
    }

    fn set(&mut self, x: u16, y: u16, cell: Cell) {
        let index = self.index(x, y);
        self.cells[index] = Some(cell);
    }

    fn index(&self, x: u16, y: u16) -> usize {
        usize::from(y) * usize::from(self.width) + usize::from(x)
    }
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> Rgb {
    let scaled_hue = hue.rem_euclid(1.0) * 6.0;
    let sector = scaled_hue.floor() as u8;
    let fraction = scaled_hue - f32::from(sector);
    let low = value * (1.0 - saturation);
    let descending = value * (1.0 - saturation * fraction);
    let ascending = value * (1.0 - saturation * (1.0 - fraction));

    let (red, green, blue) = match sector {
        0 => (value, ascending, low),
        1 => (descending, value, low),
        2 => (low, value, ascending),
        3 => (low, descending, value),
        4 => (ascending, low, value),
        _ => (value, low, descending),
    };

    Rgb {
        red: (red * 255.0).round() as u8,
        green: (green * 255.0).round() as u8,
        blue: (blue * 255.0).round() as u8,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{hsv_to_rgb, Frame, Rgb};

    #[test]
    fn primary_hues_convert_to_rgb() {
        assert_eq!(
            hsv_to_rgb(0.0, 1.0, 1.0),
            Rgb {
                red: 255,
                green: 0,
                blue: 0
            }
        );
        assert_eq!(
            hsv_to_rgb(1.0 / 3.0, 1.0, 1.0),
            Rgb {
                red: 0,
                green: 255,
                blue: 0
            }
        );
        assert_eq!(
            hsv_to_rgb(2.0 / 3.0, 1.0, 1.0),
            Rgb {
                red: 0,
                green: 0,
                blue: 255
            }
        );
    }

    #[test]
    fn frame_is_deterministic_for_a_fixed_time() {
        let elapsed = Duration::from_millis(750);
        assert_eq!(
            Frame::at_time(80, 24, elapsed),
            Frame::at_time(80, 24, elapsed)
        );
    }

    #[test]
    fn animation_changes_over_time() {
        assert_ne!(
            Frame::at_time(80, 24, Duration::ZERO),
            Frame::at_time(80, 24, Duration::from_millis(500))
        );
    }

    #[test]
    fn frames_use_the_requested_dimensions_and_ascii_glyphs() {
        for (width, height) in [(1, 1), (12, 4), (80, 24), (240, 40)] {
            let frame = Frame::at_time(width, height, Duration::from_secs(1));
            assert_eq!(frame.width(), width);
            assert_eq!(frame.height(), height);

            for y in 0..height {
                for x in 0..width {
                    if let Some(cell) = frame.cell(x, y) {
                        assert!(cell.glyph.is_ascii());
                    }
                }
            }
        }
    }

    #[test]
    fn zero_sized_frames_are_empty() {
        let frame = Frame::at_time(0, 0, Duration::ZERO);
        assert_eq!(frame.width(), 0);
        assert_eq!(frame.height(), 0);
    }
}
