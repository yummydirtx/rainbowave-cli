use std::{f32::consts::TAU, time::Duration};

use crate::audio::{AudioFrame, BAND_COUNT};

const RIBBON_COUNT: usize = 5;
const RIBBON_OFFSETS: [f32; RIBBON_COUNT] = [-0.46, -0.23, 0.0, 0.23, 0.46];
const RIBBON_PHASES: [f32; RIBBON_COUNT] = [0.15, 1.32, 2.55, 3.83, 5.08];
const RIBBON_DIRECTIONS: [f32; RIBBON_COUNT] = [1.0, -0.82, 0.72, -0.91, 0.78];
const REACTIVE_HUES: [f32; RIBBON_COUNT] = [0.50, 0.59, 0.69, 0.78, 0.87];
const HISTORY_LENGTH: usize = 32;
const RIBBON_DELAYS: [usize; RIBBON_COUNT] = [24, 11, 0, 6, 18];
const SHOCKWAVE_COUNT: usize = 4;
const PARTICLE_COUNT: usize = 48;

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

#[derive(Clone, Debug)]
pub struct Frame {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
    pixels: Vec<Light>,
    glow: Vec<Light>,
    blur: Vec<Light>,
    columns: Vec<ReactiveColumn>,
    reactive: ReactiveState,
}

// Private render caches are excluded: a Frame's observable value is its image.
impl PartialEq for Frame {
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width && self.height == other.height && self.cells == other.cells
    }
}

impl Eq for Frame {}

impl Frame {
    pub fn new(width: u16, height: u16) -> Self {
        let cell_count = usize::from(width) * usize::from(height);
        let pixel_count = cell_count * 2;
        Self {
            width,
            height,
            cells: vec![Cell::default(); cell_count],
            pixels: vec![Light::default(); pixel_count],
            glow: vec![Light::default(); pixel_count],
            blur: vec![Light::default(); pixel_count],
            columns: vec![ReactiveColumn::default(); usize::from(width)],
            reactive: ReactiveState::new(),
        }
    }

    #[cfg(test)]
    pub fn at_time(width: u16, height: u16, elapsed: Duration) -> Self {
        let mut frame = Self::new(width, height);
        frame.render_at(elapsed);
        frame
    }

    /// Renders the original autonomous scene without audio-only state or feedback.
    pub fn render_at(&mut self, elapsed: Duration) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        let time = elapsed.as_secs_f32();
        let pixel_height = u32::from(self.height) * 2;
        let silent = AudioFrame::default();
        for y in 0..self.height {
            let upper_y = u32::from(y) * 2;
            for x in 0..self.width {
                let upper = shade_static_pixel(
                    u32::from(x),
                    upper_y,
                    u32::from(self.width),
                    pixel_height,
                    time,
                    &silent,
                );
                let lower = shade_static_pixel(
                    u32::from(x),
                    upper_y + 1,
                    u32::from(self.width),
                    pixel_height,
                    time,
                    &silent,
                );
                let index = self.index(x, y);
                self.cells[index] = Cell { upper, lower };
            }
        }
    }

    /// Renders a stateful spectral aurora with event geometry and emissive afterglow.
    pub fn render_with_audio(&mut self, elapsed: Duration, audio: &AudioFrame) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        let dt = self.reactive.advance(elapsed, *audio);
        let delayed =
            std::array::from_fn(|layer| self.reactive.audio_frames_ago(RIBBON_DELAYS[layer]));
        let shockwaves = self.reactive.shockwaves;
        let particles = self.reactive.particles;
        let width = u32::from(self.width);
        let pixel_height = u32::from(self.height) * 2;
        let time = elapsed.as_secs_f32();
        let pixel_step = 2.0 / pixel_height.max(2) as f32;

        for x in 0..width {
            self.columns[x as usize] =
                prepare_reactive_column(x, width, time, pixel_step, audio, &delayed, &shockwaves);
        }
        for y in 0..pixel_height {
            let vertical = normalized_coordinate(y, pixel_height);
            for x in 0..width {
                let index = y as usize * width as usize + x as usize;
                self.pixels[index] = shade_reactive_pixel(
                    x,
                    y,
                    width,
                    pixel_height,
                    vertical,
                    time,
                    audio,
                    &self.columns[x as usize],
                    &shockwaves,
                );
            }
        }
        splat_particles(&mut self.pixels, width, pixel_height, &particles);
        apply_emissive_bloom(
            &mut self.pixels,
            &mut self.glow,
            &mut self.blur,
            width,
            pixel_height,
            dt,
            audio.energy,
        );
        write_cells_from_pixels(&mut self.cells, &self.pixels, width, self.height);
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

#[derive(Clone, Copy, Debug, Default)]
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
    fn max(self, other: Self) -> Self {
        Self::new(
            self.red.max(other.red),
            self.green.max(other.green),
            self.blue.max(other.blue),
        )
    }
    fn peak(self) -> f32 {
        self.red.max(self.green).max(self.blue)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct RibbonSample {
    center: f32,
    width: f32,
    band: f32,
    transient: f32,
    hue: f32,
    opacity: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct ReactiveColumn {
    horizontal: f32,
    band: f32,
    peak: f32,
    transient: f32,
    ribbons: [RibbonSample; RIBBON_COUNT],
}

#[derive(Clone, Copy, Debug, Default)]
struct Shockwave {
    age: f32,
    lifetime: f32,
    strength: f32,
    origin_x: f32,
    stereo_width: f32,
    hue: f32,
}

impl Shockwave {
    fn alive(self) -> bool {
        self.strength > 0.0 && self.age < self.lifetime
    }
    fn progress(self) -> f32 {
        (self.age / self.lifetime.max(0.001)).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Particle {
    x: f32,
    y: f32,
    velocity_x: f32,
    velocity_y: f32,
    age: f32,
    lifetime: f32,
    strength: f32,
    hue: f32,
    radius: f32,
}

impl Particle {
    fn alive(self) -> bool {
        self.strength > 0.0 && self.age < self.lifetime
    }
}

#[derive(Clone, Debug)]
struct ReactiveState {
    history: [AudioFrame; HISTORY_LENGTH],
    history_cursor: usize,
    history_count: usize,
    previous_time: Option<Duration>,
    previous_onsets: [f32; 3],
    shockwaves: [Shockwave; SHOCKWAVE_COUNT],
    particles: [Particle; PARTICLE_COUNT],
    event_counter: u32,
}

impl ReactiveState {
    fn new() -> Self {
        Self {
            history: [AudioFrame::default(); HISTORY_LENGTH],
            history_cursor: 0,
            history_count: 0,
            previous_time: None,
            previous_onsets: [0.0; 3],
            shockwaves: [Shockwave::default(); SHOCKWAVE_COUNT],
            particles: [Particle::default(); PARTICLE_COUNT],
            event_counter: 0,
        }
    }

    fn advance(&mut self, elapsed: Duration, audio: AudioFrame) -> f32 {
        let dt = self
            .previous_time
            .map(|previous| elapsed.saturating_sub(previous).as_secs_f32())
            .unwrap_or(1.0 / 60.0)
            .clamp(1.0 / 240.0, 0.1);
        self.previous_time = Some(elapsed);
        for wave in &mut self.shockwaves {
            if wave.alive() {
                wave.age += dt;
            }
        }
        for particle in &mut self.particles {
            if !particle.alive() {
                continue;
            }
            particle.age += dt;
            let curl_x = (particle.y * 3.1 + particle.age * 1.7).sin() * 0.12;
            let curl_y = -(particle.x * 2.7 - particle.age * 1.3).sin() * 0.08;
            particle.velocity_x += curl_x * dt;
            particle.velocity_y += curl_y * dt;
            let damping = (-dt * 1.35).exp();
            particle.velocity_x *= damping;
            particle.velocity_y *= damping;
            particle.x += particle.velocity_x * dt;
            particle.y += particle.velocity_y * dt;
        }

        let onsets = [audio.onset_low, audio.onset_mid, audio.onset_high];
        let triggers: [bool; 3] = std::array::from_fn(|index| {
            let current = onsets[index].clamp(0.0, 1.0);
            current > 0.12 && current > self.previous_onsets[index] + 0.055
        });
        if triggers[0] {
            self.spawn_shockwave(audio);
        }
        if triggers[1] {
            self.spawn_particles(audio, false);
        }
        if triggers[2] {
            self.spawn_particles(audio, true);
        }
        self.previous_onsets = onsets;
        self.history[self.history_cursor] = audio;
        self.history_cursor = (self.history_cursor + 1) % HISTORY_LENGTH;
        self.history_count = (self.history_count + 1).min(HISTORY_LENGTH);
        dt
    }

    fn audio_frames_ago(&self, frames: usize) -> AudioFrame {
        if self.history_count == 0 {
            return AudioFrame::default();
        }
        let delay = frames.min(self.history_count - 1);
        let index = (self.history_cursor + HISTORY_LENGTH - 1 - delay) % HISTORY_LENGTH;
        self.history[index]
    }

    fn spawn_shockwave(&mut self, audio: AudioFrame) {
        let slot = self
            .shockwaves
            .iter()
            .position(|wave| !wave.alive())
            .unwrap_or_else(|| {
                self.shockwaves
                    .iter()
                    .enumerate()
                    .max_by(|(_, left), (_, right)| left.age.total_cmp(&right.age))
                    .map_or(0, |(index, _)| index)
            });
        let strength = (audio.onset_low * 0.72 + audio.beat * 0.45).clamp(0.18, 1.0);
        self.shockwaves[slot] = Shockwave {
            age: 0.0,
            lifetime: 0.48 + strength * 0.34,
            strength,
            origin_x: audio.stereo_balance.clamp(-1.0, 1.0) * 0.42,
            stereo_width: audio.stereo_width.clamp(0.0, 1.0),
            hue: 0.965 - audio.centroid.clamp(0.0, 1.0) * 0.055,
        };
    }

    fn spawn_particles(&mut self, audio: AudioFrame, high: bool) {
        let onset = if high {
            audio.onset_high
        } else {
            audio.onset_mid
        }
        .clamp(0.0, 1.0);
        let count = if high {
            5 + (onset * 9.0).round() as usize
        } else {
            3 + (onset * 6.0).round() as usize
        };
        for ordinal in 0..count {
            let seed = self
                .event_counter
                .wrapping_mul(0x9E37_79B9)
                .wrapping_add((ordinal as u32).wrapping_mul(0x85EB_CA6B));
            let random_a = hash_1d(seed);
            let random_b = hash_1d(seed.wrapping_add(0x68BC_21EB));
            let random_c = hash_1d(seed.wrapping_add(0x02E5_BE93));
            let slot = self
                .particles
                .iter()
                .position(|particle| !particle.alive())
                .unwrap_or_else(|| {
                    self.particles
                        .iter()
                        .enumerate()
                        .max_by(|(_, left), (_, right)| left.age.total_cmp(&right.age))
                        .map_or(0, |(index, _)| index)
                });
            let spread = 0.13 + audio.stereo_width.clamp(0.0, 1.0) * 0.27;
            let balance = audio.stereo_balance.clamp(-1.0, 1.0) * 0.48;
            let (x, y, speed, hue, radius, lifetime) = if high {
                let side = if random_a > 0.5 { 1.0 } else { -1.0 };
                (
                    (balance + side * (0.50 + random_b * 0.42)).clamp(-1.05, 1.05),
                    -0.28 - random_c * 0.54,
                    0.42 + random_b * 0.42,
                    0.52 + random_a * 0.16,
                    0.75 + random_c * 0.72,
                    0.28 + random_b * 0.34,
                )
            } else {
                (
                    (balance + (random_a - 0.5) * spread * 2.0).clamp(-1.0, 1.0),
                    -0.12 + (random_b - 0.5) * 0.38,
                    0.25 + random_c * 0.30,
                    0.72 + random_a * 0.13,
                    1.15 + random_b * 1.05,
                    0.40 + random_c * 0.42,
                )
            };
            let angle = -2.35 + random_a * 1.55;
            self.particles[slot] = Particle {
                x,
                y,
                velocity_x: angle.cos() * speed + balance * 0.08,
                velocity_y: angle.sin() * speed,
                age: 0.0,
                lifetime,
                strength: (0.36 + onset * 0.92).min(1.25),
                hue,
                radius,
            };
            self.event_counter = self.event_counter.wrapping_add(1);
        }
    }
}

fn prepare_reactive_column(
    x: u32,
    width: u32,
    time: f32,
    pixel_step: f32,
    audio: &AudioFrame,
    delayed: &[AudioFrame; RIBBON_COUNT],
    shockwaves: &[Shockwave; SHOCKWAVE_COUNT],
) -> ReactiveColumn {
    let horizontal = normalized_coordinate(x, width);
    let spectrum_position = horizontal.abs();
    let band = spectrum_at(&audio.bands, spectrum_position);
    let peak = spectrum_at(&audio.peaks, spectrum_position).max(band);
    let transient = spectrum_at(&audio.transients, spectrum_position);
    let mut ribbons = [RibbonSample::default(); RIBBON_COUNT];

    for layer in 0..RIBBON_COUNT {
        let historical = delayed[layer];
        let local_band = spectrum_at(&historical.bands, spectrum_position);
        let local_transient = spectrum_at(&historical.transients, spectrum_position);
        let phase = horizontal * (2.64 + layer as f32 * 0.17)
            + time * (0.61 + layer as f32 * 0.041) * RIBBON_DIRECTIONS[layer]
            + RIBBON_PHASES[layer];
        let broad = phase.sin()
            * (0.15 + layer as f32 * 0.006)
            * (1.0 + historical.bass * 0.45 + audio.beat * 0.08);
        let fold = (horizontal * (5.7 + layer as f32 * 0.36)
            - time * (0.31 + layer as f32 * 0.031)
            + RIBBON_PHASES[layer] * 1.71)
            .sin()
            * (0.031 + historical.mid * 0.046);
        let detail = (horizontal * (9.4 + layer as f32 * 0.73)
            + time * RIBBON_DIRECTIONS[layer] * 1.23
            + RIBBON_PHASES[layer] * 2.1)
            .sin()
            * (local_band.powf(0.72) * 0.052 + local_transient * 0.038);
        let spectral_relief = (local_band - historical.energy * 0.23)
            * (0.072 + historical.treble * 0.025)
            * RIBBON_DIRECTIONS[layer].signum();
        let pitch_elevation = -spectrum_position * local_band * 0.035;
        let mut shock_displacement = 0.0;
        for wave in shockwaves.iter().copied().filter(|wave| wave.alive()) {
            let progress = wave.progress();
            let radius = progress * (1.12 + wave.stereo_width * 0.28);
            let front = ((horizontal - wave.origin_x).abs() - radius) / 0.105;
            shock_displacement += (-front * front).exp()
                * wave.strength
                * (1.0 - progress).powf(0.72)
                * 0.092
                * RIBBON_DIRECTIONS[layer].signum();
        }
        let depth = RIBBON_DELAYS[layer] as f32 / (HISTORY_LENGTH - 1) as f32;
        let stereo_shift =
            audio.stereo_balance.clamp(-1.0, 1.0) * (0.026 + (layer as f32 - 2.0) * 0.006);
        let spread = 1.0 + audio.bass * 0.24 + audio.stereo_width * 0.09 + audio.beat * 0.07;
        let center = RIBBON_OFFSETS[layer] * spread
            + broad
            + fold
            + detail
            + spectral_relief
            + pitch_elevation
            + shock_displacement
            + stereo_shift;
        let width = (0.014 + layer as f32 * 0.0012).max(pixel_step * 0.60)
            * (1.0 + local_band * 0.66 + historical.loudness * 0.20);
        let hue = REACTIVE_HUES[layer]
            + (audio.centroid.clamp(0.0, 1.0) - 0.5) * 0.13
            + horizontal * 0.042
            - time * 0.006
            + depth * 0.035;
        ribbons[layer] = RibbonSample {
            center,
            width,
            band: local_band,
            transient: local_transient,
            hue,
            opacity: 1.0 - depth * 0.38,
        };
    }

    ReactiveColumn {
        horizontal,
        band,
        peak,
        transient,
        ribbons,
    }
}

#[allow(clippy::too_many_arguments)]
fn shade_reactive_pixel(
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    vertical: f32,
    time: f32,
    audio: &AudioFrame,
    column: &ReactiveColumn,
    shockwaves: &[Shockwave; SHOCKWAVE_COUNT],
) -> Light {
    let horizontal = column.horizontal;
    let aspect_ratio = width as f32 / height.max(1) as f32;
    let pixel_step = 2.0 / height.max(2) as f32;
    let mut light = reactive_deep_space(horizontal, vertical, time, audio, column.band);

    // Bass lives at the center and progressively higher bands move toward the edges.
    let envelope =
        0.035 + column.band.powf(0.72) * (0.255 + audio.loudness * 0.075) + audio.bass * 0.018;
    let peak_envelope = 0.045 + column.peak.powf(0.72) * 0.36;
    let edge_distance = (vertical.abs() - envelope).abs();
    let peak_distance = (vertical.abs() - peak_envelope).abs();
    let edge = (-(edge_distance / (pixel_step * 1.15 + 0.012)).powi(2)).exp();
    let peak_ghost = (-(peak_distance / (pixel_step * 1.45 + 0.018)).powi(2)).exp();
    let interior = (1.0 - vertical.abs() / envelope.max(0.025))
        .clamp(0.0, 1.0)
        .powi(2);
    let spectrum_hue =
        0.49 + horizontal.abs() * 0.24 + (audio.centroid.clamp(0.0, 1.0) - 0.5) * 0.10;
    let spectrum_color = rainbow(spectrum_hue);
    light.add(
        spectrum_color,
        interior * column.band * (0.052 + audio.energy * 0.055),
    );
    light.add(
        spectrum_color,
        edge * (0.25 + column.band * 0.58 + column.transient * 0.45),
    );
    light.add(
        rainbow(spectrum_hue + 0.12),
        peak_ghost * column.peak * 0.13,
    );
    light.add(
        Light::new(0.84, 0.96, 1.0),
        edge.powi(3) * (0.16 + column.transient * 0.54),
    );

    // High pitch maps to a fine elevated arc; low pitch stays larger and lower.
    let pitch_height = 0.24 - horizontal.abs() * 0.92;
    let pitch_arc = (-((vertical - pitch_height) / (0.075 + pixel_step)).powi(2)).exp();
    light.add(
        rainbow(0.54 + horizontal.abs() * 0.20),
        pitch_arc * column.band * (0.035 + audio.treble * 0.075),
    );

    // The five ribbons are delayed spectral snapshots, so attacks travel through depth.
    for (layer, ribbon) in column.ribbons.iter().enumerate() {
        let distance = (vertical - ribbon.center).abs();
        let halo = (-distance * (7.0 + layer as f32 * 0.42)).exp();
        let core = (-(distance / ribbon.width).powi(2) * 1.18).exp();
        let traveling = 0.68
            + 0.32
                * (horizontal * (10.4 + layer as f32 * 0.43) - time * (1.78 + layer as f32 * 0.13)
                    + RIBBON_PHASES[layer])
                    .sin();
        let filament_phase = distance * (96.0 + audio.treble * 30.0)
            - horizontal * (7.5 + layer as f32)
            + time * RIBBON_DIRECTIONS[layer] * 1.62;
        let filament = (0.5 + 0.5 * filament_phase.cos()).powi(12) * halo;
        let color = rainbow(ribbon.hue);
        let impact = ribbon.transient * (0.25 + traveling * 0.75);
        light.add(
            color,
            halo * ribbon.opacity * (0.085 + audio.energy * 0.055),
        );
        light.add(
            color,
            filament * ribbon.opacity * (0.14 + audio.treble * 0.24 + impact * 0.38),
        );
        light.add(
            color,
            core * ribbon.opacity * (0.72 + traveling * 0.38 + ribbon.band * 0.68),
        );
        light.add(
            Light::new(0.88, 0.96, 1.0),
            core.powi(4) * ribbon.opacity * (0.28 + traveling * 0.20 + ribbon.transient * 0.72),
        );
    }

    // Bass impacts persist as expanding, stereo-positioned rings.
    for wave in shockwaves.iter().copied().filter(|wave| wave.alive()) {
        let progress = wave.progress();
        let dx = (horizontal - wave.origin_x) * aspect_ratio;
        let dy = vertical * (1.0 - wave.stereo_width * 0.16);
        let radius = progress * (0.72 + wave.stereo_width * 0.34);
        let distance = (dx * dx + dy * dy).sqrt();
        let thickness = 0.025 + progress * 0.052 + pixel_step;
        let ring = (-((distance - radius) / thickness).powi(2)).exp()
            * (1.0 - progress).powf(0.62)
            * wave.strength;
        light.add(rainbow(wave.hue), ring * 0.88);
        light.add(Light::new(1.0, 0.72, 0.56), ring.powi(2) * 0.42);
    }

    let star_seed = hash_2d(x, y);
    if star_seed > 0.994 - audio.treble * 0.004 - audio.onset_high * 0.003 {
        let phase = hash_2d(x.wrapping_add(91), y.wrapping_add(47)) * TAU;
        let speed = 1.0 + hash_2d(x.wrapping_add(17), y.wrapping_add(131)) * 2.1;
        let twinkle = (0.5 + 0.5 * (time * speed + phase).sin()).powi(8);
        light.add(
            rainbow(0.50 + star_seed * 0.25),
            0.05 + twinkle * (0.74 + audio.treble * 1.2),
        );
    }
    let vignette =
        (1.0 - (horizontal * horizontal * 0.13 + vertical * vertical * 0.19)).clamp(0.50, 1.0);
    light.scale(vignette)
}

fn reactive_deep_space(
    horizontal: f32,
    vertical: f32,
    time: f32,
    audio: &AudioFrame,
    local_band: f32,
) -> Light {
    let warp = (horizontal * 2.1 + vertical * 2.8 - time * 0.11).sin() * 0.64;
    let folded = horizontal * 2.6
        + (vertical * 3.7 + warp - time * 0.13).sin() * 0.73
        + (horizontal * -1.4 + vertical * 2.2 + time * 0.08 + warp * 0.4).sin() * 0.46;
    let cloud = (0.5 + 0.5 * (folded - time * 0.045).sin()).powi(5);
    let horizon = (-vertical.abs() * (1.32 - audio.bass * 0.17)).exp();
    let vertical_fold =
        (0.5 + 0.5 * (vertical * 5.2 + horizontal * 1.7 + time * 0.10).sin()).powi(6);
    let hue = 0.59 + (audio.centroid.clamp(0.0, 1.0) - 0.5) * 0.08 + horizontal * 0.025;
    let mut light = Light::new(0.0012, 0.0024, 0.0105);
    light.add(Light::new(0.004, 0.008, 0.031), horizon);
    light.add(
        rainbow(hue),
        0.006
            + cloud * horizon * (0.020 + audio.energy * 0.030)
            + vertical_fold * local_band * (0.006 + audio.loudness * 0.016),
    );
    light
}

fn splat_particles(pixels: &mut [Light], width: u32, height: u32, particles: &[Particle]) {
    for particle in particles
        .iter()
        .copied()
        .filter(|particle| particle.alive())
    {
        let progress = particle.age / particle.lifetime.max(0.001);
        let fade = (1.0 - progress).clamp(0.0, 1.0).powf(0.72);
        let color = rainbow(particle.hue);
        for trail in (0..3).rev() {
            let lag = trail as f32 * 0.030;
            let trail_fade = 1.0 - trail as f32 * 0.27;
            splat_light(
                pixels,
                width,
                height,
                particle.x - particle.velocity_x * lag,
                particle.y - particle.velocity_y * lag,
                particle.radius * (1.0 + trail as f32 * 0.24),
                color,
                particle.strength * fade * trail_fade,
            );
        }
        splat_light(
            pixels,
            width,
            height,
            particle.x,
            particle.y,
            particle.radius * 0.55,
            Light::new(0.92, 0.98, 1.0),
            particle.strength * fade * 1.45,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn splat_light(
    pixels: &mut [Light],
    width: u32,
    height: u32,
    normalized_x: f32,
    normalized_y: f32,
    radius: f32,
    color: Light,
    strength: f32,
) {
    if width == 0 || height == 0 {
        return;
    }
    let center_x = (normalized_x * 0.5 + 0.5) * (width - 1) as f32;
    let center_y = (normalized_y * 0.5 + 0.5) * (height - 1) as f32;
    let extent = (radius * 2.8).ceil() as i32;
    let minimum_x = (center_x.floor() as i32 - extent).max(0);
    let maximum_x = (center_x.ceil() as i32 + extent).min(width as i32 - 1);
    let minimum_y = (center_y.floor() as i32 - extent).max(0);
    let maximum_y = (center_y.ceil() as i32 + extent).min(height as i32 - 1);
    let inverse_radius = 1.0 / radius.max(0.25);
    for y in minimum_y..=maximum_y {
        for x in minimum_x..=maximum_x {
            let dx = (x as f32 - center_x) * inverse_radius;
            let dy = (y as f32 - center_y) * inverse_radius;
            let gaussian = (-(dx * dx + dy * dy) * 1.35).exp();
            let index = y as usize * width as usize + x as usize;
            pixels[index].add(color, gaussian * strength);
        }
    }
}

fn apply_emissive_bloom(
    pixels: &mut [Light],
    glow: &mut [Light],
    blur: &mut [Light],
    width: u32,
    height: u32,
    dt: f32,
    energy: f32,
) {
    if width == 0 || height == 0 {
        return;
    }
    let persistence = (-dt / (0.16 + energy.clamp(0.0, 1.0) * 0.08)).exp();
    for ((source, history), temporary) in pixels.iter().zip(glow.iter_mut()).zip(blur.iter_mut()) {
        let peak = source.peak();
        let extraction = ((peak - 0.24) / peak.max(0.001)).clamp(0.0, 1.0);
        *history = source.scale(extraction).max(history.scale(persistence));
        *temporary = Light::default();
    }

    // Feedback is applied only to bright emission, keeping the deep-space blacks crisp.
    const OFFSETS: [i32; 5] = [-2, -1, 0, 1, 2];
    const WEIGHTS: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            let mut sum = Light::default();
            for (offset, weight) in OFFSETS.into_iter().zip(WEIGHTS) {
                let sample_x = (x + offset).clamp(0, width as i32 - 1);
                sum.add(
                    glow[y as usize * width as usize + sample_x as usize],
                    weight,
                );
            }
            blur[y as usize * width as usize + x as usize] = sum;
        }
    }
    let bloom_strength = 0.30 + energy.clamp(0.0, 1.0) * 0.17;
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            let mut sum = Light::default();
            for (offset, weight) in OFFSETS.into_iter().zip(WEIGHTS) {
                let sample_y = (y + offset).clamp(0, height as i32 - 1);
                sum.add(
                    blur[sample_y as usize * width as usize + x as usize],
                    weight,
                );
            }
            pixels[y as usize * width as usize + x as usize].add(sum, bloom_strength);
        }
    }
}

fn write_cells_from_pixels(cells: &mut [Cell], pixels: &[Light], width: u32, height: u16) {
    for y in 0..u32::from(height) {
        for x in 0..width {
            let upper_index = (y * 2) as usize * width as usize + x as usize;
            cells[y as usize * width as usize + x as usize] = Cell {
                upper: to_rgb(pixels[upper_index]),
                lower: to_rgb(pixels[upper_index + width as usize]),
            };
        }
    }
}

fn spectrum_at(values: &[f32; BAND_COUNT], position: f32) -> f32 {
    let scaled = position.clamp(0.0, 1.0) * (BAND_COUNT - 1) as f32;
    let index = scaled.floor() as usize;
    let fraction = scaled - index as f32;
    let previous = values[index.saturating_sub(1)];
    let current = values[index];
    let next = values[(index + 1).min(BAND_COUNT - 1)];
    let following = values[(index + 2).min(BAND_COUNT - 1)];
    let squared = fraction * fraction;
    let cubed = squared * fraction;
    (0.5 * (2.0 * current
        + (-previous + next) * fraction
        + (2.0 * previous - 5.0 * current + 4.0 * next - following) * squared
        + (-previous + 3.0 * current - 3.0 * next + following) * cubed))
        .clamp(0.0, 1.5)
}

fn shade_static_pixel(
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    time: f32,
    audio: &AudioFrame,
) -> Rgb {
    let horizontal = normalized_coordinate(x, width);
    let vertical = normalized_coordinate(y, height);
    let aspect_ratio = width as f32 / height.max(1) as f32;
    let pixel_step = 2.0 / height.max(2) as f32;
    let local_band = audio.band_at(horizontal.abs());
    let mut light = deep_space(horizontal, vertical, time, audio);

    for layer in 0..RIBBON_COUNT {
        let center = ribbon_center(layer, horizontal, time, audio);
        let distance = (vertical - center).abs();
        let core_width = (0.015 + layer as f32 * 0.0015).max(pixel_step * 0.62)
            * (1.0 + local_band * 0.58 + audio.beat * 0.16);
        let halo = (-distance * (7.2 + layer as f32 * 0.35) * (1.0 - audio.energy * 0.08)).exp();
        let core = (-(distance / core_width).powi(2) * 1.25).exp();
        let traveling =
            0.72 + 0.28 * (horizontal * 11.0 - time * 2.15 + RIBBON_PHASES[layer]).sin();
        let echo_phase = distance * 94.0 - horizontal * (7.0 + layer as f32)
            + time * RIBBON_DIRECTIONS[layer] * 1.65;
        let echo = (0.5 + 0.5 * echo_phase.cos()).powi(10) * halo;
        let hue = horizontal * 0.34 + 0.5 - time * 0.045 + layer as f32 * 0.127 + center * 0.035;
        let spectral = rainbow(hue);
        light.add(spectral, halo * (0.105 + audio.energy * 0.055));
        light.add(spectral, echo * (0.16 + audio.treble * 0.18));
        light.add(
            spectral,
            core * (0.92 + traveling * 0.42 + local_band * 0.46 + audio.beat * 0.34),
        );
        light.add(
            Light::new(1.0, 0.93, 1.0),
            core.powi(4) * (0.42 + traveling * 0.28 + local_band * 0.3 + audio.beat * 0.38),
        );
    }
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
    let star_seed = hash_2d(x, y);
    if star_seed > 0.992 - audio.treble * 0.0025 {
        let phase = hash_2d(x.wrapping_add(91), y.wrapping_add(47)) * TAU;
        let speed = 0.7 + hash_2d(x.wrapping_add(17), y.wrapping_add(131)) * 1.3;
        let twinkle = (0.5 + 0.5 * (time * speed + phase).sin()).powi(6);
        let brightness = 0.08 + twinkle * 1.45 + audio.treble * (0.28 + twinkle * 1.4);
        light.add(rainbow(star_seed * 4.7 + time * 0.012), brightness);
    }
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
    let channel = |offset: f32| (0.5 + 0.5 * (angle + offset).cos()).powi(2) * 0.97 + 0.03;
    Light::new(channel(0.0), channel(-TAU / 3.0), channel(TAU / 3.0))
}

fn hash_1d(mut value: u32) -> f32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7FEB_352D);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846C_A68B);
    value ^= value >> 16;
    value as f32 / u32::MAX as f32
}

fn hash_2d(x: u32, y: u32) -> f32 {
    hash_1d(
        x.wrapping_mul(0x9E37_79B9)
            .wrapping_add(y.wrapping_mul(0x85EB_CA6B)),
    )
}

fn to_rgb(light: Light) -> Rgb {
    fn channel(value: f32) -> u8 {
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

    use crate::audio::{AudioFrame, BAND_COUNT};

    use super::{rainbow, Frame};

    fn reactive_audio() -> AudioFrame {
        AudioFrame {
            bands: std::array::from_fn(|index| {
                let position = index as f32 / (BAND_COUNT - 1) as f32;
                0.42 + (position * std::f32::consts::TAU * 2.0).sin().abs() * 0.48
            }),
            peaks: [0.94; BAND_COUNT],
            transients: std::array::from_fn(
                |index| {
                    if index > BAND_COUNT / 2 {
                        0.72
                    } else {
                        0.18
                    }
                },
            ),
            energy: 0.72,
            loudness: 0.68,
            bass: 0.80,
            mid: 0.61,
            treble: 0.77,
            onset_low: 0.92,
            onset_mid: 0.74,
            onset_high: 0.86,
            beat: 1.0,
            centroid: 0.64,
            stereo_balance: 0.22,
            stereo_width: 0.73,
        }
    }

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
        reactive.render_with_audio(elapsed, &reactive_audio());
        assert_ne!(still, reactive);
    }

    #[test]
    fn low_and_high_spectra_create_distinct_compositions() {
        let elapsed = Duration::from_millis(900);
        let mut low = Frame::new(80, 24);
        let mut high = Frame::new(80, 24);
        let mut low_audio = AudioFrame::default();
        let mut high_audio = AudioFrame::default();
        low_audio.bands[..BAND_COUNT / 3].fill(0.9);
        low_audio.peaks = low_audio.bands;
        low_audio.energy = 0.45;
        low_audio.loudness = 0.45;
        low_audio.bass = 0.9;
        high_audio.bands[BAND_COUNT * 2 / 3..].fill(0.9);
        high_audio.peaks = high_audio.bands;
        high_audio.energy = 0.45;
        high_audio.loudness = 0.45;
        high_audio.treble = 0.9;
        high_audio.centroid = 0.9;
        low.render_with_audio(elapsed, &low_audio);
        high.render_with_audio(elapsed, &high_audio);
        assert_ne!(low, high);
    }

    #[test]
    fn transient_geometry_persists_after_the_trigger() {
        let mut with_history = Frame::new(80, 24);
        with_history.render_with_audio(Duration::ZERO, &reactive_audio());
        with_history.render_with_audio(Duration::from_millis(180), &AudioFrame::default());
        let mut fresh = Frame::new(80, 24);
        fresh.render_with_audio(Duration::from_millis(180), &AudioFrame::default());
        assert_ne!(with_history, fresh);
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
    fn reactive_scene_keeps_highlight_headroom() {
        let mut frame = Frame::new(100, 30);
        frame.render_with_audio(Duration::from_millis(1_250), &reactive_audio());
        let frame_ref = &frame;
        let nearly_white = (0..frame.height())
            .flat_map(|y| (0..frame.width()).map(move |x| frame_ref.cell(x, y)))
            .flat_map(|cell| [cell.upper, cell.lower])
            .filter(|color| color.red > 245 && color.green > 245 && color.blue > 245)
            .count();
        assert!(
            nearly_white < 350,
            "{nearly_white} pixels clipped near white"
        );
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
