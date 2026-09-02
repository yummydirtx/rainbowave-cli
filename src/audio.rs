use std::{
    f32::consts::TAU,
    io,
    sync::{
        atomic::{fence, AtomicU32, Ordering},
        Arc,
    },
};

pub const BAND_COUNT: usize = 24;
const FFT_SIZE: usize = 2_048;
const HOP_SIZE: usize = 512;
const SPECTRUM_BINS: usize = FFT_SIZE / 2 + 1;
const MIN_FREQUENCY: f32 = 40.0;
const MAX_FREQUENCY: f32 = 16_000.0;
const LOW_BAND_END: usize = 8;
const MID_BAND_END: usize = 18;

const PEAKS_OFFSET: usize = BAND_COUNT;
const TRANSIENTS_OFFSET: usize = BAND_COUNT * 2;
const SCALARS_OFFSET: usize = BAND_COUNT * 3;
const SCALAR_COUNT: usize = 12;
const FRAME_VALUE_COUNT: usize = SCALARS_OFFSET + SCALAR_COUNT;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioFrame {
    pub bands: [f32; BAND_COUNT],
    pub peaks: [f32; BAND_COUNT],
    pub transients: [f32; BAND_COUNT],
    pub energy: f32,
    pub loudness: f32,
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub onset_low: f32,
    pub onset_mid: f32,
    pub onset_high: f32,
    pub beat: f32,
    pub centroid: f32,
    pub stereo_balance: f32,
    pub stereo_width: f32,
}

impl AudioFrame {
    /// Samples the spectrum from bass at `0.0` to treble at `1.0`.
    pub fn band_at(&self, position: f32) -> f32 {
        sample_curve(&self.bands, position)
    }

    fn to_values(self) -> [f32; FRAME_VALUE_COUNT] {
        let mut values = [0.0; FRAME_VALUE_COUNT];
        values[..BAND_COUNT].copy_from_slice(&self.bands);
        values[PEAKS_OFFSET..TRANSIENTS_OFFSET].copy_from_slice(&self.peaks);
        values[TRANSIENTS_OFFSET..SCALARS_OFFSET].copy_from_slice(&self.transients);
        values[SCALARS_OFFSET..].copy_from_slice(&[
            self.energy,
            self.loudness,
            self.bass,
            self.mid,
            self.treble,
            self.onset_low,
            self.onset_mid,
            self.onset_high,
            self.beat,
            self.centroid,
            self.stereo_balance,
            self.stereo_width,
        ]);
        values
    }

    fn from_values(values: [f32; FRAME_VALUE_COUNT]) -> Self {
        let mut bands = [0.0; BAND_COUNT];
        let mut peaks = [0.0; BAND_COUNT];
        let mut transients = [0.0; BAND_COUNT];
        bands.copy_from_slice(&values[..BAND_COUNT]);
        peaks.copy_from_slice(&values[PEAKS_OFFSET..TRANSIENTS_OFFSET]);
        transients.copy_from_slice(&values[TRANSIENTS_OFFSET..SCALARS_OFFSET]);
        Self {
            bands,
            peaks,
            transients,
            energy: values[SCALARS_OFFSET],
            loudness: values[SCALARS_OFFSET + 1],
            bass: values[SCALARS_OFFSET + 2],
            mid: values[SCALARS_OFFSET + 3],
            treble: values[SCALARS_OFFSET + 4],
            onset_low: values[SCALARS_OFFSET + 5],
            onset_mid: values[SCALARS_OFFSET + 6],
            onset_high: values[SCALARS_OFFSET + 7],
            beat: values[SCALARS_OFFSET + 8],
            centroid: values[SCALARS_OFFSET + 9],
            stereo_balance: values[SCALARS_OFFSET + 10],
            stereo_width: values[SCALARS_OFFSET + 11],
        }
    }
}

fn sample_curve(values: &[f32; BAND_COUNT], position: f32) -> f32 {
    let scaled = position.clamp(0.0, 1.0) * (BAND_COUNT - 1) as f32;
    let lower = scaled.floor() as usize;
    let upper = (lower + 1).min(BAND_COUNT - 1);
    let mix = scaled - lower as f32;

    values[lower] + (values[upper] - values[lower]) * mix
}

struct SharedAudio {
    sequence: AtomicU32,
    values: [AtomicU32; FRAME_VALUE_COUNT],
}

impl SharedAudio {
    fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
            values: std::array::from_fn(|_| AtomicU32::new(0.0_f32.to_bits())),
        }
    }

    fn publish(&self, frame: AudioFrame) {
        self.sequence.fetch_add(1, Ordering::AcqRel);
        for (destination, value) in self.values.iter().zip(frame.to_values()) {
            destination.store(value.to_bits(), Ordering::Relaxed);
        }
        self.sequence.fetch_add(1, Ordering::Release);
    }

    fn snapshot(&self) -> AudioFrame {
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if before % 2 != 0 {
                std::hint::spin_loop();
                continue;
            }

            let values = std::array::from_fn(|index| {
                f32::from_bits(self.values[index].load(Ordering::Relaxed))
            });
            // Keep all value loads ahead of the validating sequence load. Combined
            // with the writer's release, this turns the atomics into a compact seqlock.
            fence(Ordering::Acquire);
            let after = self.sequence.load(Ordering::Relaxed);
            if before == after {
                return AudioFrame::from_values(values);
            }
        }
    }
}

pub struct AudioCapture {
    shared: Arc<SharedAudio>,
    _backend: platform::Capture,
}

impl AudioCapture {
    pub fn start() -> io::Result<Self> {
        let shared = Arc::new(SharedAudio::new());
        let backend = platform::Capture::start(Arc::clone(&shared))?;
        Ok(Self {
            shared,
            _backend: backend,
        })
    }

    pub fn snapshot(&self) -> AudioFrame {
        self.shared.snapshot()
    }
}

struct Analyzer {
    shared: Arc<SharedAudio>,
    sample_rate: f32,
    left_history: [f32; FFT_SIZE],
    right_history: [f32; FFT_SIZE],
    write_index: usize,
    frames_seen: usize,
    frames_since_analysis: usize,
    window: [f32; FFT_SIZE],
    window_sum: f32,
    fft_real: [f32; FFT_SIZE],
    fft_imaginary: [f32; FFT_SIZE],
    level_envelope: f32,
    loudness: f32,
    smoothed_bands: [f32; BAND_COUNT],
    peaks: [f32; BAND_COUNT],
    transients: [f32; BAND_COUNT],
    previous_targets: [f32; BAND_COUNT],
    onset_floor: [f32; 3],
    onsets: [f32; 3],
    beat: f32,
    beat_cooldown: usize,
    centroid: f32,
    stereo_balance: f32,
    stereo_width: f32,
}

impl Analyzer {
    fn new(sample_rate: u32, shared: Arc<SharedAudio>) -> Self {
        let sample_rate = sample_rate.max(1) as f32;
        let window: [f32; FFT_SIZE] = std::array::from_fn(|index| {
            let phase = TAU * index as f32 / (FFT_SIZE - 1) as f32;
            0.42 - 0.5 * phase.cos() + 0.08 * (phase * 2.0).cos()
        });
        Self {
            shared,
            sample_rate,
            left_history: [0.0; FFT_SIZE],
            right_history: [0.0; FFT_SIZE],
            write_index: 0,
            frames_seen: 0,
            frames_since_analysis: 0,
            window_sum: window.iter().sum(),
            window,
            fft_real: [0.0; FFT_SIZE],
            fft_imaginary: [0.0; FFT_SIZE],
            level_envelope: 0.006,
            loudness: 0.0,
            smoothed_bands: [0.0; BAND_COUNT],
            peaks: [0.0; BAND_COUNT],
            transients: [0.0; BAND_COUNT],
            previous_targets: [0.0; BAND_COUNT],
            onset_floor: [0.0; 3],
            onsets: [0.0; 3],
            beat: 0.0,
            beat_cooldown: 0,
            centroid: 0.0,
            stereo_balance: 0.0,
            stereo_width: 0.0,
        }
    }

    fn push_interleaved(&mut self, samples: &[f32], channels: usize) {
        if channels == 0 {
            return;
        }

        for frame in samples.chunks_exact(channels) {
            let left = sanitize_sample(frame[0]);
            let right = if channels > 1 {
                sanitize_sample(frame[1])
            } else {
                left
            };
            self.push_stereo_frame(left, right);
        }
    }

    fn push_stereo_frame(&mut self, left: f32, right: f32) {
        self.left_history[self.write_index] = left;
        self.right_history[self.write_index] = right;
        self.write_index = (self.write_index + 1) % FFT_SIZE;
        self.frames_seen = self.frames_seen.saturating_add(1);
        self.frames_since_analysis += 1;

        if self.frames_seen >= FFT_SIZE && self.frames_since_analysis >= HOP_SIZE {
            self.frames_since_analysis = 0;
            self.analyze_window();
        }
    }

    fn analyze_window(&mut self) {
        let (raw_loudness, raw_balance, raw_width) =
            stereo_features(&self.left_history, &self.right_history);
        let active = raw_loudness > 0.000_01;

        prepare_fft(
            &self.left_history,
            self.write_index,
            &self.window,
            &mut self.fft_real,
            &mut self.fft_imaginary,
        );
        fft_in_place(&mut self.fft_real, &mut self.fft_imaginary);

        let scale = 2.0 / self.window_sum;
        let power_scale = scale * scale * 0.5;
        let mut spectrum = [0.0; SPECTRUM_BINS];
        for (bin, power) in spectrum.iter_mut().enumerate() {
            *power = (self.fft_real[bin] * self.fft_real[bin]
                + self.fft_imaginary[bin] * self.fft_imaginary[bin])
                * power_scale;
        }

        prepare_fft(
            &self.right_history,
            self.write_index,
            &self.window,
            &mut self.fft_real,
            &mut self.fft_imaginary,
        );
        fft_in_place(&mut self.fft_real, &mut self.fft_imaginary);
        for (bin, power) in spectrum.iter_mut().enumerate() {
            *power += (self.fft_real[bin] * self.fft_real[bin]
                + self.fft_imaginary[bin] * self.fft_imaginary[bin])
                * power_scale;
        }

        let band_powers = logarithmic_bands(&spectrum, self.sample_rate);
        self.level_envelope = raw_loudness.max(self.level_envelope * 0.996).max(0.006);
        let automatic_gain = (0.30 / self.level_envelope).clamp(0.65, 40.0);
        let targets: [f32; BAND_COUNT] = std::array::from_fn(|index| {
            if active {
                let position = index as f32 / (BAND_COUNT - 1) as f32;
                let perceptual_weight = 0.88 + 1.02 * position.powf(0.65);
                let amplitude = band_powers[index].sqrt();
                (1.0 - (-amplitude * automatic_gain * perceptual_weight * 2.8).exp())
                    .clamp(0.0, 1.0)
            } else {
                0.0
            }
        });

        // Keep a fixed dB-domain loudness lane alongside the adaptive spectrum. Quiet
        // passages retain spectral detail without becoming as visually massive as a chorus.
        let loudness_target = normalized_loudness(raw_loudness);
        let loudness_response = if loudness_target > self.loudness {
            0.54
        } else {
            0.035
        };
        self.loudness += (loudness_target - self.loudness) * loudness_response;

        // Compare against a local spectral maximum so a sustained tone sliding into an
        // adjacent band does not masquerade as a fresh onset.
        let positive_flux: [f32; BAND_COUNT] = std::array::from_fn(|index| {
            let first = index.saturating_sub(1);
            let last = (index + 1).min(BAND_COUNT - 1);
            let reference = maximum(&self.previous_targets[first..=last]);
            (targets[index] - reference).max(0.0)
        });
        for index in 0..BAND_COUNT {
            let band_response = if targets[index] > self.smoothed_bands[index] {
                0.52
            } else {
                0.095
            };
            self.smoothed_bands[index] +=
                (targets[index] - self.smoothed_bands[index]) * band_response;
            self.peaks[index] = (self.peaks[index] * 0.982).max(self.smoothed_bands[index]);
            self.transients[index] =
                (self.transients[index] * 0.78).max((positive_flux[index] * 3.4).min(1.0));
        }

        let fluxes = [
            mean(&positive_flux[..LOW_BAND_END]),
            mean(&positive_flux[LOW_BAND_END..MID_BAND_END]),
            mean(&positive_flux[MID_BAND_END..]),
        ];
        for (index, flux) in fluxes.into_iter().enumerate() {
            self.onset_floor[index] = self.onset_floor[index] * 0.96 + flux * 0.04;
            let target = ((flux - self.onset_floor[index] * 1.18) * 7.5).clamp(0.0, 1.0);
            self.onsets[index] = (self.onsets[index] * 0.78).max(target);
        }

        self.beat *= 0.88;
        if self.beat_cooldown > 0 {
            self.beat_cooldown -= 1;
        } else {
            let beat_onset =
                (self.onsets[0] * 0.72 + self.onsets[1] * 0.22 + self.onsets[2] * 0.06)
                    .max(self.onsets[0] * 0.88);
            if beat_onset > 0.12 && raw_loudness > 0.000_1 {
                self.beat = self.beat.max((0.42 + beat_onset * 0.72).min(1.0));
                self.beat_cooldown = 8;
            }
        }

        self.previous_targets = targets;

        let spectral_power = band_powers.iter().sum::<f32>();
        let centroid_target = if active && spectral_power > f32::EPSILON {
            band_powers
                .iter()
                .enumerate()
                .map(|(index, power)| power * index as f32 / (BAND_COUNT - 1) as f32)
                .sum::<f32>()
                / spectral_power
        } else {
            0.0
        };
        self.centroid += (centroid_target - self.centroid) * if active { 0.28 } else { 0.08 };
        self.stereo_balance +=
            (raw_balance - self.stereo_balance) * if active { 0.34 } else { 0.08 };
        self.stereo_width += (raw_width - self.stereo_width) * if active { 0.30 } else { 0.08 };

        let spectral_density = root_mean_square(&self.smoothed_bands);
        let energy = (self.loudness * 0.72 + spectral_density * 0.28).clamp(0.0, 1.0);
        self.shared.publish(AudioFrame {
            bands: self.smoothed_bands,
            peaks: self.peaks,
            transients: self.transients,
            energy,
            loudness: self.loudness.clamp(0.0, 1.0),
            bass: group_level(&self.smoothed_bands[..LOW_BAND_END]),
            mid: group_level(&self.smoothed_bands[LOW_BAND_END..MID_BAND_END]),
            treble: group_level(&self.smoothed_bands[MID_BAND_END..]),
            onset_low: self.onsets[0].clamp(0.0, 1.0),
            onset_mid: self.onsets[1].clamp(0.0, 1.0),
            onset_high: self.onsets[2].clamp(0.0, 1.0),
            beat: self.beat,
            centroid: self.centroid.clamp(0.0, 1.0),
            stereo_balance: self.stereo_balance.clamp(-1.0, 1.0),
            stereo_width: self.stereo_width.clamp(0.0, 1.0),
        });
    }
}

fn sanitize_sample(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-8.0, 8.0)
    } else {
        0.0
    }
}

fn normalized_loudness(root_mean_square: f32) -> f32 {
    if root_mean_square <= 0.000_01 {
        return 0.0;
    }

    let decibels = 20.0 * root_mean_square.log10();
    let linear = ((decibels + 70.0) / 64.0).clamp(0.0, 1.0);
    linear * linear * (3.0 - 2.0 * linear)
}

fn prepare_fft(
    history: &[f32; FFT_SIZE],
    oldest: usize,
    window: &[f32; FFT_SIZE],
    real: &mut [f32; FFT_SIZE],
    imaginary: &mut [f32; FFT_SIZE],
) {
    for index in 0..FFT_SIZE {
        real[index] = history[(oldest + index) % FFT_SIZE] * window[index];
    }
    imaginary.fill(0.0);
}

fn fft_in_place(real: &mut [f32; FFT_SIZE], imaginary: &mut [f32; FFT_SIZE]) {
    let mut reversed = 0;
    for index in 1..FFT_SIZE {
        let mut bit = FFT_SIZE >> 1;
        while reversed & bit != 0 {
            reversed ^= bit;
            bit >>= 1;
        }
        reversed ^= bit;
        if index < reversed {
            real.swap(index, reversed);
            imaginary.swap(index, reversed);
        }
    }

    let mut length = 2;
    while length <= FFT_SIZE {
        let (step_sine, step_cosine) = (-TAU / length as f32).sin_cos();
        for block in (0..FFT_SIZE).step_by(length) {
            let mut twiddle_real = 1.0;
            let mut twiddle_imaginary = 0.0;
            for offset in 0..length / 2 {
                let even = block + offset;
                let odd = even + length / 2;
                let odd_real = twiddle_real * real[odd] - twiddle_imaginary * imaginary[odd];
                let odd_imaginary = twiddle_real * imaginary[odd] + twiddle_imaginary * real[odd];

                real[odd] = real[even] - odd_real;
                imaginary[odd] = imaginary[even] - odd_imaginary;
                real[even] += odd_real;
                imaginary[even] += odd_imaginary;

                let next_real = twiddle_real * step_cosine - twiddle_imaginary * step_sine;
                twiddle_imaginary = twiddle_real * step_sine + twiddle_imaginary * step_cosine;
                twiddle_real = next_real;
            }
        }
        length *= 2;
    }
}

fn logarithmic_bands(spectrum: &[f32; SPECTRUM_BINS], sample_rate: f32) -> [f32; BAND_COUNT] {
    let mut powers = [0.0; BAND_COUNT];
    let logarithmic_span = (MAX_FREQUENCY / MIN_FREQUENCY).ln();
    let band_ratio = (logarithmic_span / (BAND_COUNT - 1) as f32).exp();
    let lower_edge = MIN_FREQUENCY / band_ratio.sqrt();
    let upper_edge = MAX_FREQUENCY * band_ratio.sqrt();
    let maximum_frequency = upper_edge.min(sample_rate * 0.5);

    for (bin, power) in spectrum.iter().copied().enumerate().skip(1) {
        let frequency = bin as f32 * sample_rate / FFT_SIZE as f32;
        if frequency < lower_edge || frequency > maximum_frequency {
            continue;
        }

        let position =
            (frequency / MIN_FREQUENCY).ln() / logarithmic_span * (BAND_COUNT - 1) as f32;
        if position <= 0.0 {
            powers[0] += power;
        } else if position >= (BAND_COUNT - 1) as f32 {
            powers[BAND_COUNT - 1] += power;
        } else {
            let lower = position.floor() as usize;
            let fraction = position - lower as f32;
            powers[lower] += power * (1.0 - fraction);
            powers[lower + 1] += power * fraction;
        }
    }

    powers
}

fn stereo_features(left: &[f32; FFT_SIZE], right: &[f32; FFT_SIZE]) -> (f32, f32, f32) {
    let mut left_power = 0.0;
    let mut right_power = 0.0;
    let mut mid_power = 0.0;
    let mut side_power = 0.0;
    for (&left, &right) in left.iter().zip(right) {
        left_power += left * left;
        right_power += right * right;
        let mid = (left + right) * 0.5;
        let side = (left - right) * 0.5;
        mid_power += mid * mid;
        side_power += side * side;
    }

    let divisor = FFT_SIZE as f32;
    left_power /= divisor;
    right_power /= divisor;
    mid_power /= divisor;
    side_power /= divisor;
    let channel_power = left_power + right_power;
    if channel_power <= f32::EPSILON {
        return (0.0, 0.0, 0.0);
    }

    let loudness = (channel_power * 0.5).sqrt();
    let balance = (right_power - left_power) / channel_power;
    let width = side_power / (mid_power + side_power).max(f32::EPSILON);
    (
        loudness.max(0.0),
        balance.clamp(-1.0, 1.0),
        width.clamp(0.0, 1.0),
    )
}

fn mean(values: &[f32]) -> f32 {
    values.iter().sum::<f32>() / values.len() as f32
}

fn maximum(values: &[f32]) -> f32 {
    values.iter().copied().fold(0.0, f32::max)
}

fn root_mean_square(values: &[f32]) -> f32 {
    (values.iter().map(|value| value * value).sum::<f32>() / values.len() as f32).sqrt()
}

fn group_level(values: &[f32]) -> f32 {
    (mean(values) * 0.58 + maximum(values) * 0.42).clamp(0.0, 1.0)
}

#[cfg(target_os = "macos")]
mod platform {
    use std::{
        ffi::{c_char, c_void, CStr},
        io,
        process::Command,
        ptr::NonNull,
        slice,
        sync::Arc,
    };

    use super::{Analyzer, SharedAudio};

    pub struct Capture {
        handle: NonNull<c_void>,
        _analyzer: Box<Analyzer>,
    }

    impl Capture {
        pub fn start(shared: Arc<SharedAudio>) -> io::Result<Self> {
            require_supported_macos()?;

            eprintln!(
                "rainbowave: choose Entire Screen for all system audio, or choose the apps to visualize"
            );
            let mut analyzer = Box::new(Analyzer::new(48_000, shared));
            let context = analyzer.as_mut() as *mut Analyzer as *mut c_void;
            let mut error = std::ptr::null_mut();
            let handle =
                unsafe { rainbowave_screen_audio_start(receive_samples, context, &mut error) };
            let handle = NonNull::new(handle).ok_or_else(|| capture_error(error))?;

            Ok(Self {
                handle,
                _analyzer: analyzer,
            })
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            // The native stop waits for ScreenCaptureKit's serial sample queue to drain,
            // so `_analyzer` remains valid until no callback can reach it.
            unsafe { rainbowave_screen_audio_stop(self.handle.as_ptr()) };
        }
    }

    unsafe extern "C" fn receive_samples(
        context: *mut c_void,
        samples: *const f32,
        sample_count: usize,
        channel_count: usize,
    ) {
        if context.is_null() || samples.is_null() || sample_count == 0 || channel_count == 0 {
            return;
        }

        let analyzer = unsafe { &mut *context.cast::<Analyzer>() };
        let samples = unsafe { slice::from_raw_parts(samples, sample_count) };
        analyzer.push_interleaved(samples, channel_count);
    }

    fn require_supported_macos() -> io::Result<()> {
        let output = Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .map_err(|error| {
                io::Error::other(format!("could not determine the macOS version ({error})"))
            })?;
        let version = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() || !supports_system_audio(version.trim()) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "the system audio sharing picker requires macOS 14.0 or newer (this Mac reports {})",
                    version.trim()
                ),
            ));
        }

        Ok(())
    }

    fn supports_system_audio(version: &str) -> bool {
        let mut components = version.split('.');
        let major = components
            .next()
            .and_then(|value| value.parse::<u32>().ok());
        let minor = components
            .next()
            .and_then(|value| value.parse::<u32>().ok());

        matches!((major, minor), (Some(major), Some(_minor)) if major >= 14)
    }

    fn capture_error(error: *mut c_char) -> io::Error {
        let message = if error.is_null() {
            "unknown ScreenCaptureKit error".to_owned()
        } else {
            let message = unsafe { CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned();
            unsafe { rainbowave_screen_audio_error_free(error) };
            message
        };
        io::Error::other(message)
    }

    type SamplesCallback = unsafe extern "C" fn(*mut c_void, *const f32, usize, usize);

    extern "C" {
        fn rainbowave_screen_audio_start(
            callback: SamplesCallback,
            context: *mut c_void,
            error: *mut *mut c_char,
        ) -> *mut c_void;
        fn rainbowave_screen_audio_stop(handle: *mut c_void);
        fn rainbowave_screen_audio_error_free(error: *mut c_char);
    }

    #[cfg(test)]
    mod tests {
        use super::supports_system_audio;

        #[test]
        fn audio_version_requirement_is_explicit() {
            for supported in ["14.0", "14.6.1", "15.0", "26.0"] {
                assert!(supports_system_audio(supported), "{supported}");
            }
            for unsupported in ["12.7", "13.0", "13.9.9", "unknown"] {
                assert!(!supports_system_audio(unsupported), "{unsupported}");
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::{
        io::{self, Read},
        mem::size_of,
        process::{Child, Command, Stdio},
        sync::Arc,
        thread::{self, JoinHandle},
        time::Duration,
    };

    use super::{Analyzer, SharedAudio};

    const SAMPLE_RATE: u32 = 48_000;
    const CHANNELS: usize = 2;

    pub struct Capture {
        child: Child,
        reader: Option<JoinHandle<()>>,
        error_reader: Option<JoinHandle<()>>,
    }

    impl Capture {
        pub fn start(shared: Arc<SharedAudio>) -> io::Result<Self> {
            let mut child = Command::new("parec")
                .args([
                    "--raw",
                    "--format=float32le",
                    "--rate=48000",
                    "--channels=2",
                    "--latency-msec=20",
                    "--device=@DEFAULT_MONITOR@",
                    "--client-name=rainbowave",
                    "--stream-name=Rainbowave system audio",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| {
                    io::Error::new(
                        error.kind(),
                        format!(
                            "could not start `parec` ({error}); install PulseAudio utilities (usually the `pulseaudio-utils` package)"
                        ),
                    )
                })?;

            thread::sleep(Duration::from_millis(40));
            if let Some(status) = child.try_wait()? {
                let mut details = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    stderr.read_to_string(&mut details)?;
                }
                let details = details.trim();
                let suffix = if details.is_empty() {
                    String::new()
                } else {
                    format!(": {details}")
                };
                return Err(io::Error::other(format!(
                    "`parec` could not capture the default PulseAudio/PipeWire monitor ({status}){suffix}"
                )));
            }

            let mut stdout = child.stdout.take().ok_or_else(|| {
                io::Error::other("`parec` started without a readable audio stream")
            })?;
            let reader = thread::spawn(move || {
                let mut analyzer = Analyzer::new(SAMPLE_RATE, shared);
                let mut bytes = [0_u8; 8_192];
                let mut decoded = [0.0_f32; 2_048];
                let mut remainder = Vec::with_capacity(bytes.len() + 7);

                while let Ok(count) = stdout.read(&mut bytes) {
                    if count == 0 {
                        break;
                    }
                    remainder.extend_from_slice(&bytes[..count]);
                    let bytes_per_frame = size_of::<f32>() * CHANNELS;
                    let complete_bytes = remainder.len() / bytes_per_frame * bytes_per_frame;
                    let sample_count = complete_bytes / size_of::<f32>();
                    for (destination, sample) in decoded[..sample_count]
                        .iter_mut()
                        .zip(remainder[..complete_bytes].chunks_exact(size_of::<f32>()))
                    {
                        *destination =
                            f32::from_le_bytes(sample.try_into().expect("four-byte sample"));
                    }
                    analyzer.push_interleaved(&decoded[..sample_count], CHANNELS);
                    remainder.drain(..complete_bytes);
                }
            });

            let error_reader = child.stderr.take().map(|mut stderr| {
                thread::spawn(move || {
                    let _ = io::copy(&mut stderr, &mut io::sink());
                })
            });

            Ok(Self {
                child,
                reader: Some(reader),
                error_reader,
            })
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
            if let Some(error_reader) = self.error_reader.take() {
                let _ = error_reader.join();
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    use std::{io, sync::Arc};

    use super::SharedAudio;

    pub struct Capture;

    impl Capture {
        pub fn start(_shared: Arc<SharedAudio>) -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "system audio capture is currently supported on macOS and Linux",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    use super::{
        Analyzer, AudioFrame, SharedAudio, BAND_COUNT, FFT_SIZE, HOP_SIZE, LOW_BAND_END,
        MID_BAND_END,
    };

    const SAMPLE_RATE: u32 = 48_000;

    #[test]
    fn spectrum_interpolation_reaches_both_ends() {
        let frame = AudioFrame {
            bands: std::array::from_fn(|index| index as f32),
            ..AudioFrame::default()
        };

        assert_eq!(frame.band_at(0.0), 0.0);
        assert_eq!(frame.band_at(1.0), (BAND_COUNT - 1) as f32);
        assert!((frame.band_at(0.5) - 11.5).abs() < f32::EPSILON);
    }

    #[test]
    fn low_tones_favor_low_frequency_bands() {
        let frame = analyze_tone(110.0);
        let low = maximum(&frame.bands[..LOW_BAND_END]);
        let high = maximum(&frame.bands[MID_BAND_END..]);

        assert!(low > high * 4.0, "low={low}, high={high}, {frame:?}");
    }

    #[test]
    fn high_tones_favor_high_frequency_bands() {
        let frame = analyze_tone(6_000.0);
        let low = maximum(&frame.bands[..LOW_BAND_END]);
        let high = maximum(&frame.bands[MID_BAND_END..]);

        assert!(high > low * 4.0, "low={low}, high={high}, {frame:?}");
    }

    #[test]
    fn spectral_centroid_tracks_pitch_on_a_logarithmic_axis() {
        let low = analyze_tone(110.0);
        let high = analyze_tone(6_000.0);

        assert!(
            high.centroid > low.centroid + 0.45,
            "low={}, high={}",
            low.centroid,
            high.centroid
        );
        assert!(low.centroid < 0.3, "{low:?}");
        assert!(high.centroid > 0.7, "{high:?}");
    }

    #[test]
    fn adaptive_spectrum_keeps_detail_without_flattening_loudness() {
        let quiet = analyze_tone_at_amplitude(440.0, 0.012);
        let loud = analyze_tone_at_amplitude(440.0, 0.48);

        assert!(maximum(&quiet.bands) > 0.35, "{quiet:?}");
        assert!(
            loud.loudness > quiet.loudness + 0.35,
            "quiet={}, loud={}",
            quiet.loudness,
            loud.loudness
        );
    }

    #[test]
    fn silence_remains_dark() {
        let shared = Arc::new(SharedAudio::new());
        let mut analyzer = Analyzer::new(SAMPLE_RATE, Arc::clone(&shared));
        analyzer.push_interleaved(&vec![0.0; FFT_SIZE + HOP_SIZE * 4], 1);

        assert_eq!(shared.snapshot(), AudioFrame::default());
    }

    #[test]
    fn anti_phase_stereo_retains_spectral_energy() {
        let frame = analyze_stereo_tone(220.0, 0.35, 0.45, -0.45);

        assert!(frame.energy > 0.35, "{frame:?}");
        assert!(maximum(&frame.bands) > 0.55, "{frame:?}");
        assert!(frame.stereo_balance.abs() < 0.03, "{frame:?}");
        assert!(frame.stereo_width > 0.92, "{frame:?}");
    }

    #[test]
    fn stereo_balance_and_width_preserve_spatial_information() {
        let left = analyze_stereo_tone(440.0, 0.35, 0.4, 0.0);
        let right = analyze_stereo_tone(440.0, 0.35, 0.0, 0.4);
        let center = analyze_stereo_tone(440.0, 0.35, 0.4, 0.4);

        assert!(left.stereo_balance < -0.9, "{left:?}");
        assert!(right.stereo_balance > 0.9, "{right:?}");
        assert!(center.stereo_balance.abs() < 0.03, "{center:?}");
        assert!(center.stereo_width < 0.03, "{center:?}");
        assert!(left.stereo_width > 0.45 && left.stereo_width < 0.55);
    }

    #[test]
    fn a_new_transient_produces_onset_and_beat_then_decays() {
        let shared = Arc::new(SharedAudio::new());
        let mut analyzer = Analyzer::new(SAMPLE_RATE, Arc::clone(&shared));
        analyzer.push_interleaved(&vec![0.0; FFT_SIZE], 1);
        analyzer.push_interleaved(&tone_frames(90.0, HOP_SIZE * 2, 0.8), 1);
        let attack = shared.snapshot();

        analyzer.push_interleaved(&tone_frames(90.0, HOP_SIZE * 14, 0.8), 1);
        let sustained = shared.snapshot();

        assert!(attack.onset_low > 0.15, "{attack:?}");
        assert!(attack.beat > 0.2, "{attack:?}");
        assert!(maximum(&attack.transients) > 0.25, "{attack:?}");
        assert!(
            sustained.onset_low < attack.onset_low * 0.35,
            "{sustained:?}"
        );
        assert!(
            maximum(&sustained.transients) < maximum(&attack.transients) * 0.35,
            "attack={attack:?}, sustained={sustained:?}"
        );
    }

    #[test]
    fn onset_lanes_distinguish_low_mid_and_high_attacks() {
        let low = analyze_attack(90.0);
        let mid = analyze_attack(1_000.0);
        let high = analyze_attack(8_000.0);

        assert!(
            low.onset_low > low.onset_mid.max(low.onset_high) * 1.5,
            "{low:?}"
        );
        assert!(
            mid.onset_mid > mid.onset_low.max(mid.onset_high) * 1.5,
            "{mid:?}"
        );
        assert!(
            high.onset_high > high.onset_low.max(high.onset_mid) * 1.5,
            "{high:?}"
        );
    }

    #[test]
    fn spectral_peaks_fall_more_slowly_than_live_bands() {
        let shared = Arc::new(SharedAudio::new());
        let mut analyzer = Analyzer::new(SAMPLE_RATE, Arc::clone(&shared));
        analyzer.push_interleaved(&tone(880.0, 0.2, 0.5), 1);
        let active_peak = maximum(&shared.snapshot().peaks);
        analyzer.push_interleaved(&vec![0.0; FFT_SIZE + HOP_SIZE * 2], 1);
        let release = shared.snapshot();

        assert!(active_peak > 0.5);
        assert!(maximum(&release.peaks) > maximum(&release.bands) * 1.25);
        assert!(maximum(&release.peaks) < active_peak);
    }

    #[test]
    fn shared_snapshot_round_trips_a_complete_frame() {
        let shared = SharedAudio::new();
        let expected = AudioFrame {
            bands: std::array::from_fn(|index| index as f32 / BAND_COUNT as f32),
            peaks: std::array::from_fn(|index| 1.0 - index as f32 / BAND_COUNT as f32),
            transients: std::array::from_fn(|index| (index % 3) as f32 * 0.2),
            energy: 0.42,
            loudness: 0.61,
            bass: 0.3,
            mid: 0.4,
            treble: 0.5,
            onset_low: 0.2,
            onset_mid: 0.4,
            onset_high: 0.6,
            beat: 0.9,
            centroid: 0.73,
            stereo_balance: -0.36,
            stereo_width: 0.81,
        };
        shared.publish(expected);

        assert_eq!(shared.snapshot(), expected);
    }

    #[test]
    fn concurrent_shared_snapshots_never_tear() {
        let shared = Arc::new(SharedAudio::new());
        let barrier = Arc::new(Barrier::new(2));
        let writer_shared = Arc::clone(&shared);
        let writer_barrier = Arc::clone(&barrier);
        let writer = thread::spawn(move || {
            writer_barrier.wait();
            for index in 1..=50_000 {
                let value = (index % 997) as f32 / 997.0;
                writer_shared.publish(uniform_frame(value));
            }
        });

        barrier.wait();
        for _ in 0..50_000 {
            let values = shared.snapshot().to_values();
            assert!(values.iter().all(|value| *value == values[0]));
        }
        writer.join().expect("writer completes");
    }

    #[test]
    fn hostile_samples_produce_only_finite_clamped_features() {
        let shared = Arc::new(SharedAudio::new());
        let mut analyzer = Analyzer::new(SAMPLE_RATE, Arc::clone(&shared));
        let pattern = [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            1.0e30,
            -1.0e30,
            0.25,
        ];
        let samples: Vec<_> = pattern
            .into_iter()
            .cycle()
            .take((FFT_SIZE + HOP_SIZE * 3) * 2)
            .collect();
        analyzer.push_interleaved(&samples, 2);
        let frame = shared.snapshot();

        assert!(frame.to_values().into_iter().all(f32::is_finite));
        for value in frame
            .bands
            .into_iter()
            .chain(frame.peaks)
            .chain(frame.transients)
            .chain([
                frame.energy,
                frame.loudness,
                frame.bass,
                frame.mid,
                frame.treble,
                frame.onset_low,
                frame.onset_mid,
                frame.onset_high,
                frame.beat,
                frame.centroid,
                frame.stereo_width,
            ])
        {
            assert!((0.0..=1.0).contains(&value), "value={value}, {frame:?}");
        }
        assert!((-1.0..=1.0).contains(&frame.stereo_balance));
    }

    fn analyze_tone(frequency: f32) -> AudioFrame {
        analyze_tone_at_amplitude(frequency, 0.35)
    }

    fn analyze_tone_at_amplitude(frequency: f32, amplitude: f32) -> AudioFrame {
        let shared = Arc::new(SharedAudio::new());
        let mut analyzer = Analyzer::new(SAMPLE_RATE, Arc::clone(&shared));
        analyzer.push_interleaved(&tone(frequency, 0.35, amplitude), 1);
        shared.snapshot()
    }

    fn analyze_stereo_tone(
        frequency: f32,
        seconds: f32,
        left_amplitude: f32,
        right_amplitude: f32,
    ) -> AudioFrame {
        let shared = Arc::new(SharedAudio::new());
        let mut analyzer = Analyzer::new(SAMPLE_RATE, Arc::clone(&shared));
        let frame_count = (SAMPLE_RATE as f32 * seconds) as usize;
        let mut samples = Vec::with_capacity(frame_count * 2);
        for index in 0..frame_count {
            let wave =
                (std::f32::consts::TAU * frequency * index as f32 / SAMPLE_RATE as f32).sin();
            samples.push(wave * left_amplitude);
            samples.push(wave * right_amplitude);
        }
        analyzer.push_interleaved(&samples, 2);
        shared.snapshot()
    }

    fn analyze_attack(frequency: f32) -> AudioFrame {
        let shared = Arc::new(SharedAudio::new());
        let mut analyzer = Analyzer::new(SAMPLE_RATE, Arc::clone(&shared));
        analyzer.push_interleaved(&vec![0.0; FFT_SIZE], 1);
        analyzer.push_interleaved(&tone_frames(frequency, HOP_SIZE * 2, 0.7), 1);
        shared.snapshot()
    }

    fn tone(frequency: f32, seconds: f32, amplitude: f32) -> Vec<f32> {
        let sample_count = (SAMPLE_RATE as f32 * seconds) as usize;
        (0..sample_count)
            .map(|index| {
                (std::f32::consts::TAU * frequency * index as f32 / SAMPLE_RATE as f32).sin()
                    * amplitude
            })
            .collect()
    }

    fn tone_frames(frequency: f32, frame_count: usize, amplitude: f32) -> Vec<f32> {
        (0..frame_count)
            .map(|index| {
                (std::f32::consts::TAU * frequency * index as f32 / SAMPLE_RATE as f32).sin()
                    * amplitude
            })
            .collect()
    }

    fn maximum(values: &[f32]) -> f32 {
        values.iter().copied().fold(0.0, f32::max)
    }

    fn uniform_frame(value: f32) -> AudioFrame {
        AudioFrame {
            bands: [value; BAND_COUNT],
            peaks: [value; BAND_COUNT],
            transients: [value; BAND_COUNT],
            energy: value,
            loudness: value,
            bass: value,
            mid: value,
            treble: value,
            onset_low: value,
            onset_mid: value,
            onset_high: value,
            beat: value,
            centroid: value,
            stereo_balance: value,
            stereo_width: value,
        }
    }
}
