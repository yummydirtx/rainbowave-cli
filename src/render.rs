use std::{f32::consts::TAU, time::Duration};

use crate::audio::AudioFrame;

const RIBBON_COUNT: usize = 5;
const RIBBON_OFFSETS: [f32; RIBBON_COUNT] = [-0.46, -0.23, 0.0, 0.23, 0.46];
const RIBBON_PHASES: [f32; RIBBON_COUNT] = [0.15, 1.32, 2.55, 3.83, 5.08];
const RIBBON_DIRECTIONS: [f32; RIBBON_COUNT] = [1.0, -0.82, 0.72, -0.91, 0.78];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Cell {
    /// Color for the upper half of the terminal cell.
    pub upper: Rgb,
    /// Color for the lower half of the terminal cell.
    pub lower: Rgb,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
}

impl Frame {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            cells: vec![Cell::default(); usize::from(width) * usize::from(height)],
        }
    }

    #[cfg(test)]
    pub fn at_time(width: u16, height: u16, elapsed: Duration) -> Self {
        let mut frame = Self::new(width, height);
        frame.render_at(elapsed);
        frame
    }

    pub fn render_at(&mut self, elapsed: Duration) {
        self.render_with_audio(elapsed, &AudioFrame::default());
    }

    pub fn render_with_audio(&mut self, elapsed: Duration, audio: &AudioFrame) {
        if self.width == 0 || self.height == 0 {
            return;
        }

        let time = elapsed.as_secs_f32();
        let pixel_height = u32::from(self.height) * 2;

        for y in 0..self.height {
            let upper_y = u32::from(y) * 2;
            let lower_y = upper_y + 1;

            for x in 0..self.width {
                let upper = shade_pixel(
                    u32::from(x),
                    upper_y,
                    u32::from(self.width),
                    pixel_height,
                    time,
                    audio,
                );
                let lower = shade_pixel(
                    u32::from(x),
                    lower_y,
                    u32::from(self.width),
                    pixel_height,
                    time,
                    audio,
                );
                let index = self.index(x, y);
                self.cells[index] = Cell { upper, lower };
            }
        }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn cell(&self, x: u16, y: u16) -> Cell {
        self.cells[self.index(x, y)]
    }

    fn index(&self, x: u16, y: u16) -> usize {
        usize::from(y) * usize::from(self.width) + usize::from(x)
    }
}

#[derive(Clone, Copy, Debug)]
struct Light {
    red: f32,
    green: f32,
    blue: f32,
}

impl Light {
    const fn new(red: f32, green: f32, blue: f32) -> Self {
        Self { red, green, blue }
    }

    fn add(&mut self, other: Self, intensity: f32) {
        self.red += other.red * intensity;
        self.green += other.green * intensity;
        self.blue += other.blue * intensity;
    }

    fn scale(self, factor: f32) -> Self {
        Self::new(self.red * factor, self.green * factor, self.blue * factor)
    }
}

fn shade_pixel(x: u32, y: u32, width: u32, height: u32, time: f32, audio: &AudioFrame) -> Rgb {
    let horizontal = normalized_coordinate(x, width);
    let vertical = normalized_coordinate(y, height);
    let aspect_ratio = width as f32 / height.max(1) as f32;
    let pixel_step = 2.0 / height.max(2) as f32;

    let local_band = audio.band_at(horizontal.abs());
    let mut light = deep_space(horizontal, vertical, time, audio);

    // Each translucent ribbon has a wide halo, a textured body, and a hot core. The
    // alternating directions make the layers braid through one another instead of
    // moving like copies of the same sine wave.
    for layer in 0..RIBBON_COUNT {
        let center = ribbon_center(layer, horizontal, time, audio);
        let distance = (vertical - center).abs();
        let core_width = (0.015 + layer as f32 * 0.0015).max(pixel_step * 0.62)
            * (1.0 + local_band * 0.58 + audio.beat * 0.16);
        let halo = (-distance * (7.2 + layer as f32 * 0.35) * (1.0 - audio.energy * 0.08)).exp();
        let core = (-(distance / core_width).powi(2) * 1.25).exp();
        let traveling_pulse =
            0.72 + 0.28 * (horizontal * 11.0 - time * 2.15 + RIBBON_PHASES[layer]).sin();

        // Narrow echo filaments inside the glow give the wave visible internal motion.
        let echo_phase = distance * 94.0 - horizontal * (7.0 + layer as f32)
            + time * RIBBON_DIRECTIONS[layer] * 1.65;
        let echo = (0.5 + 0.5 * echo_phase.cos()).powi(10) * halo;
        let hue = horizontal * 0.34 + 0.5 - time * 0.045 + layer as f32 * 0.127 + center * 0.035;
        let spectral = rainbow(hue);

        light.add(spectral, halo * (0.105 + audio.energy * 0.055));
        light.add(spectral, echo * (0.16 + audio.treble * 0.18));
        light.add(
            spectral,
            core * (0.92 + traveling_pulse * 0.42 + local_band * 0.46 + audio.beat * 0.34),
        );
        light.add(
            Light::new(1.0, 0.93, 1.0),
            core.powi(4) * (0.42 + traveling_pulse * 0.28 + local_band * 0.3 + audio.beat * 0.38),
        );
    }

    // A handful of bright motes ride along the ribbons. Their aspect-correct falloff
    // keeps them round even as the terminal is resized.
    for mote in 0..4 {
        let seed = mote as f32 * 0.237 + 0.08;
        let progress = (time * (0.055 + mote as f32 * 0.006) + seed).rem_euclid(1.0);
        let mote_x = progress * 2.4 - 1.2;
        let layer = (mote * 2 + 1) % RIBBON_COUNT;
        let mote_y = ribbon_center(layer, mote_x, time, audio);
        let dx = (horizontal - mote_x) * aspect_ratio;
        let dy = vertical - mote_y;
        let distance_squared = dx * dx + dy * dy;
        let bloom = (-distance_squared * (115.0 - audio.energy * 28.0)).exp();
        let core = (-distance_squared * 1_100.0).exp();
        let color = rainbow(seed + time * 0.035);

        light.add(color, bloom * (0.72 + audio.energy * 0.72));
        light.add(
            Light::new(1.0, 0.96, 1.0),
            core * (2.4 + audio.treble * 1.6 + audio.beat * 1.2),
        );
    }

    // Sparse, deterministic points add depth to otherwise dark areas. Their slow,
    // independent twinkle makes the background feel alive without becoming noisy.
    let star_seed = hash_2d(x, y);
    if star_seed > 0.992 - audio.treble * 0.0025 {
        let phase = hash_2d(x.wrapping_add(91), y.wrapping_add(47)) * TAU;
        let speed = 0.7 + hash_2d(x.wrapping_add(17), y.wrapping_add(131)) * 1.3;
        let twinkle = (0.5 + 0.5 * (time * speed + phase).sin()).powi(6);
        let brightness = 0.08 + twinkle * 1.45 + audio.treble * (0.28 + twinkle * 1.4);
        light.add(rainbow(star_seed * 4.7 + time * 0.012), brightness);
    }

    // Darken the very edges to keep the eye on the moving field and preserve contrast.
    let vignette =
        (1.0 - (horizontal * horizontal * 0.14 + vertical * vertical * 0.22)).clamp(0.48, 1.0);
    to_rgb(light.scale(vignette))
}

fn normalized_coordinate(position: u32, extent: u32) -> f32 {
    if extent <= 1 {
        0.0
    } else {
        position as f32 / (extent - 1) as f32 * 2.0 - 1.0
    }
}

fn deep_space(horizontal: f32, vertical: f32, time: f32, audio: &AudioFrame) -> Light {
    let folded = horizontal * 2.15
        + (vertical * 3.4 - time * 0.17).sin() * 0.72
        + (horizontal * -1.3 + vertical * 2.1 + time * 0.11).sin() * 0.43;
    let cloud = (0.5 + 0.5 * (folded - time * 0.08).sin()).powi(4);
    let horizon = (-vertical.abs() * 1.45).exp();
    let mut light = Light::new(0.0015, 0.003, 0.011);
    let haze = rainbow(0.61 + horizontal * 0.035 + vertical * 0.025 - time * 0.006);

    light.add(Light::new(0.005, 0.008, 0.026), horizon);
    light.add(
        haze,
        0.008 + cloud * horizon * 0.026 + audio.energy * horizon * (0.018 + cloud * 0.045),
    );
    light
}

fn ribbon_center(layer: usize, horizontal: f32, time: f32, audio: &AudioFrame) -> f32 {
    let phase = horizontal * (2.72 + layer as f32 * 0.14)
        + time * (0.68 + layer as f32 * 0.035) * RIBBON_DIRECTIONS[layer]
        + RIBBON_PHASES[layer];
    let broad_wave =
        phase.sin() * (0.16 + layer as f32 * 0.007) * (1.0 + audio.bass * 0.34 + audio.beat * 0.1);
    let fine_wave = (horizontal * (6.1 + layer as f32 * 0.18)
        - time * (0.34 + layer as f32 * 0.025)
        + RIBBON_PHASES[layer] * 1.7)
        .sin()
        * (0.038 + audio.mid * 0.026);
    let spectral_wave = (horizontal * (8.7 + layer as f32 * 0.56)
        + time * RIBBON_DIRECTIONS[layer] * (1.4 + layer as f32 * 0.08)
        + RIBBON_PHASES[layer] * 2.3)
        .sin()
        * audio.band_at(horizontal.abs())
        * (0.026 + audio.treble * 0.012);

    RIBBON_OFFSETS[layer] * (1.0 + audio.beat * 0.09) + broad_wave + fine_wave + spectral_wave
}

fn rainbow(hue: f32) -> Light {
    let angle = hue.rem_euclid(1.0) * TAU;
    // Squaring a cosine palette keeps the transitions smooth while allowing every hue
    // to reach a saturated primary. A tiny floor prevents harsh, empty color channels.
    let channel = |offset: f32| (0.5 + 0.5 * (angle + offset).cos()).powi(2) * 0.97 + 0.03;

    Light::new(channel(0.0), channel(-TAU / 3.0), channel(TAU / 3.0))
}

fn hash_2d(x: u32, y: u32) -> f32 {
    let mut value = x
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(y.wrapping_mul(0x85EB_CA6B));
    value ^= value >> 16;
    value = value.wrapping_mul(0x7FEB_352D);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846C_A68B);
    value ^= value >> 16;
    value as f32 / u32::MAX as f32
}

fn to_rgb(light: Light) -> Rgb {
    fn channel(value: f32) -> u8 {
        // Exponential tone mapping retains detail in overlapping blooms, then converts
        // from linear light to the sRGB transfer curve expected by terminal truecolor.
        let linear = 1.0 - (-value.max(0.0) * 1.18).exp();
        let srgb = if linear <= 0.003_130_8 {
            linear * 12.92
        } else {
            1.055 * linear.powf(1.0 / 2.4) - 0.055
        };
        (srgb.clamp(0.0, 1.0) * 255.0).round() as u8
    }

    Rgb {
        red: channel(light.red),
        green: channel(light.green),
        blue: channel(light.blue),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::audio::AudioFrame;

    use super::{rainbow, Frame};

    #[test]
    fn palette_visits_all_three_primary_colors() {
        let red = rainbow(0.0);
        let green = rainbow(1.0 / 3.0);
        let blue = rainbow(2.0 / 3.0);

        assert!(red.red > red.green && red.red > red.blue);
        assert!(green.green > green.red && green.green > green.blue);
        assert!(blue.blue > blue.red && blue.blue > blue.green);
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
    fn audio_spectrum_changes_the_light_field() {
        let elapsed = Duration::from_millis(900);
        let still = Frame::at_time(80, 24, elapsed);
        let mut reactive = Frame::new(80, 24);
        reactive.render_with_audio(
            elapsed,
            &AudioFrame {
                bands: [0.9, 0.82, 0.7, 0.58, 0.5, 0.68, 0.8, 0.92],
                energy: 0.72,
                bass: 0.8,
                mid: 0.6,
                treble: 0.8,
                beat: 1.0,
            },
        );

        assert_ne!(still, reactive);
    }

    #[test]
    fn frames_use_the_requested_dimensions() {
        for (width, height) in [(1, 1), (12, 4), (80, 24), (240, 40)] {
            let frame = Frame::at_time(width, height, Duration::from_secs(1));
            assert_eq!(frame.width(), width);
            assert_eq!(frame.height(), height);
        }
    }

    #[test]
    fn half_blocks_contain_two_independent_vertical_samples() {
        let frame = Frame::at_time(80, 24, Duration::from_millis(900));
        let frame_ref = &frame;
        let differing_halves = (0..frame.height())
            .flat_map(|y| (0..frame.width()).map(move |x| frame_ref.cell(x, y)))
            .filter(|cell| cell.upper != cell.lower)
            .count();

        assert!(differing_halves > 1_000);
    }

    #[test]
    fn scene_has_both_deep_shadows_and_bright_highlights() {
        let frame = Frame::at_time(100, 30, Duration::from_millis(1_250));
        let frame_ref = &frame;
        let brightness = (0..frame.height())
            .flat_map(|y| (0..frame.width()).map(move |x| frame_ref.cell(x, y)))
            .flat_map(|cell| [cell.upper, cell.lower])
            .map(|color| u16::from(color.red) + u16::from(color.green) + u16::from(color.blue));
        let (darkest, brightest) = brightness
            .fold((u16::MAX, u16::MIN), |(darkest, brightest), value| {
                (darkest.min(value), brightest.max(value))
            });

        assert!(darkest < 160, "darkest pixel was {darkest}");
        assert!(brightest > 650, "brightest pixel was {brightest}");
    }

    #[test]
    fn zero_sized_frames_are_empty() {
        for (width, height) in [(0, 0), (0, 10), (10, 0)] {
            let frame = Frame::at_time(width, height, Duration::ZERO);
            assert_eq!(frame.width(), width);
            assert_eq!(frame.height(), height);
            assert!(frame.cells.is_empty());
        }
    }
}
