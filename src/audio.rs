use std::{
    f32::consts::TAU,
    io,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};

pub const BAND_COUNT: usize = 8;
const FRAME_VALUE_COUNT: usize = BAND_COUNT + 5;
const BAND_CENTERS: [f32; BAND_COUNT] =
    [55.0, 110.0, 220.0, 440.0, 880.0, 1_760.0, 3_520.0, 7_040.0];
const BAND_WEIGHTS: [f32; BAND_COUNT] = [0.86, 0.94, 1.02, 1.14, 1.34, 1.62, 2.02, 2.48];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioFrame {
    pub bands: [f32; BAND_COUNT],
    pub energy: f32,
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub beat: f32,
}

impl AudioFrame {
    /// Samples the spectrum from bass at `0.0` to treble at `1.0`.
    pub fn band_at(self, position: f32) -> f32 {
        let scaled = position.clamp(0.0, 1.0) * (BAND_COUNT - 1) as f32;
        let lower = scaled.floor() as usize;
        let upper = (lower + 1).min(BAND_COUNT - 1);
        let mix = scaled - lower as f32;

        self.bands[lower] + (self.bands[upper] - self.bands[lower]) * mix
    }

    fn to_values(self) -> [f32; FRAME_VALUE_COUNT] {
        let mut values = [0.0; FRAME_VALUE_COUNT];
        values[..BAND_COUNT].copy_from_slice(&self.bands);
        values[BAND_COUNT] = self.energy;
        values[BAND_COUNT + 1] = self.bass;
        values[BAND_COUNT + 2] = self.mid;
        values[BAND_COUNT + 3] = self.treble;
        values[BAND_COUNT + 4] = self.beat;
        values
    }

    fn from_values(values: [f32; FRAME_VALUE_COUNT]) -> Self {
        let mut bands = [0.0; BAND_COUNT];
        bands.copy_from_slice(&values[..BAND_COUNT]);
        Self {
            bands,
            energy: values[BAND_COUNT],
            bass: values[BAND_COUNT + 1],
            mid: values[BAND_COUNT + 2],
            treble: values[BAND_COUNT + 3],
            beat: values[BAND_COUNT + 4],
        }
    }
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
            let after = self.sequence.load(Ordering::Acquire);
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

#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn band_pass(sample_rate: f32, center_frequency: f32, quality: f32) -> Self {
        let frequency = center_frequency.min(sample_rate * 0.45);
        let omega = TAU * frequency / sample_rate;
        let alpha = omega.sin() / (2.0 * quality);
        let a0 = 1.0 + alpha;

        Self {
            b0: alpha / a0,
            b1: 0.0,
            b2: -alpha / a0,
            a1: -2.0 * omega.cos() / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn process(&mut self, sample: f32) -> f32 {
        let output = self.b0 * sample + self.z1;
        self.z1 = self.b1 * sample - self.a1 * output + self.z2;
        self.z2 = self.b2 * sample - self.a2 * output;
        output
    }
}

struct Analyzer {
    shared: Arc<SharedAudio>,
    filters: [Biquad; BAND_COUNT],
    band_squares: [f32; BAND_COUNT],
    sample_squares: f32,
    samples_in_window: usize,
    window_samples: usize,
    level_envelope: f32,
    smoothed_bands: [f32; BAND_COUNT],
    previous_targets: [f32; BAND_COUNT],
    onset_floor: f32,
    beat: f32,
    beat_cooldown: usize,
}

impl Analyzer {
    fn new(sample_rate: u32, shared: Arc<SharedAudio>) -> Self {
        let sample_rate = sample_rate.max(1) as f32;
        Self {
            shared,
            filters: std::array::from_fn(|index| {
                Biquad::band_pass(sample_rate, BAND_CENTERS[index], 0.82)
            }),
            band_squares: [0.0; BAND_COUNT],
            sample_squares: 0.0,
            samples_in_window: 0,
            window_samples: (sample_rate as usize / 100).max(1),
            level_envelope: 0.012,
            smoothed_bands: [0.0; BAND_COUNT],
            previous_targets: [0.0; BAND_COUNT],
            onset_floor: 0.0,
            beat: 0.0,
            beat_cooldown: 0,
        }
    }

    fn push_interleaved(&mut self, samples: &[f32], channels: usize) {
        if channels == 0 {
            return;
        }

        for frame in samples.chunks_exact(channels) {
            let mono = frame.iter().copied().sum::<f32>() / channels as f32;
            self.push_sample(if mono.is_finite() { mono } else { 0.0 });
        }
    }

    fn push_sample(&mut self, sample: f32) {
        self.sample_squares += sample * sample;
        for (sum, filter) in self.band_squares.iter_mut().zip(&mut self.filters) {
            let filtered = filter.process(sample);
            *sum += filtered * filtered;
        }
        self.samples_in_window += 1;

        if self.samples_in_window >= self.window_samples {
            self.finish_window();
        }
    }

    fn finish_window(&mut self) {
        let divisor = self.samples_in_window as f32;
        let raw_energy = (self.sample_squares / divisor).sqrt();

        // Follow loud passages immediately but release over several seconds. This makes
        // the visualization equally useful for quiet streams and mastered music without
        // pumping the gain between individual beats.
        self.level_envelope = raw_energy.max(self.level_envelope * 0.9975).max(0.012);
        let automatic_gain = (0.34 / self.level_envelope).clamp(1.0, 32.0);
        let active = raw_energy > 0.000_15;
        let targets = std::array::from_fn(|index| {
            if active {
                let amplitude = (self.band_squares[index] / divisor).sqrt();
                1.0 - (-amplitude * automatic_gain * BAND_WEIGHTS[index] * 4.2).exp()
            } else {
                0.0
            }
        });

        let spectral_flux = targets
            .iter()
            .zip(self.previous_targets)
            .map(|(current, previous)| (current - previous).max(0.0))
            .sum::<f32>()
            / BAND_COUNT as f32;
        let bass_target = mean(&targets[..3]);
        let previous_bass = mean(&self.previous_targets[..3]);
        self.onset_floor = self.onset_floor * 0.94 + spectral_flux * 0.06;
        let onset = (spectral_flux - self.onset_floor * 1.35).max(0.0)
            + (bass_target - previous_bass).max(0.0) * 0.45;

        self.beat *= 0.86;
        if self.beat_cooldown > 0 {
            self.beat_cooldown -= 1;
        } else if onset > 0.035 && raw_energy > 0.001 {
            self.beat = (0.3 + onset * 5.5).clamp(0.0, 1.0);
            self.beat_cooldown = 16;
        }

        for (smoothed, target) in self.smoothed_bands.iter_mut().zip(targets) {
            let response = if target > *smoothed { 0.46 } else { 0.085 };
            *smoothed += (target - *smoothed) * response;
        }
        self.previous_targets = targets;

        let energy = mean(&self.smoothed_bands).clamp(0.0, 1.0);
        self.shared.publish(AudioFrame {
            bands: self.smoothed_bands,
            energy,
            bass: mean(&self.smoothed_bands[..3]),
            mid: mean(&self.smoothed_bands[2..6]),
            treble: mean(&self.smoothed_bands[5..]),
            beat: self.beat,
        });

        self.band_squares.fill(0.0);
        self.sample_squares = 0.0;
        self.samples_in_window = 0;
    }
}

fn mean(values: &[f32]) -> f32 {
    values.iter().sum::<f32>() / values.len() as f32
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
    ) {
        if context.is_null() || samples.is_null() || sample_count == 0 {
            return;
        }

        let analyzer = unsafe { &mut *context.cast::<Analyzer>() };
        let samples = unsafe { slice::from_raw_parts(samples, sample_count) };
        analyzer.push_interleaved(samples, 1);
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

    type SamplesCallback = unsafe extern "C" fn(*mut c_void, *const f32, usize);

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
    use std::sync::Arc;

    use super::{Analyzer, AudioFrame, SharedAudio, BAND_COUNT};

    const SAMPLE_RATE: u32 = 48_000;

    #[test]
    fn spectrum_interpolation_reaches_both_ends() {
        let frame = AudioFrame {
            bands: std::array::from_fn(|index| index as f32),
            ..AudioFrame::default()
        };

        assert_eq!(frame.band_at(0.0), 0.0);
        assert_eq!(frame.band_at(1.0), (BAND_COUNT - 1) as f32);
        assert!((frame.band_at(0.5) - 3.5).abs() < f32::EPSILON);
    }

    #[test]
    fn low_tones_favor_low_frequency_bands() {
        let frame = analyze_tone(110.0);
        let low = frame.bands[..3].iter().copied().fold(0.0, f32::max);
        let high = frame.bands[5..].iter().copied().fold(0.0, f32::max);

        assert!(low > high * 1.8, "low={low}, high={high}, {frame:?}");
    }

    #[test]
    fn high_tones_favor_high_frequency_bands() {
        let frame = analyze_tone(5_000.0);
        let low = frame.bands[..3].iter().copied().fold(0.0, f32::max);
        let high = frame.bands[5..].iter().copied().fold(0.0, f32::max);

        assert!(high > low * 1.8, "low={low}, high={high}, {frame:?}");
    }

    #[test]
    fn silence_remains_dark() {
        let shared = Arc::new(SharedAudio::new());
        let mut analyzer = Analyzer::new(SAMPLE_RATE, Arc::clone(&shared));
        analyzer.push_interleaved(&vec![0.0; SAMPLE_RATE as usize / 5], 1);

        assert_eq!(shared.snapshot(), AudioFrame::default());
    }

    #[test]
    fn a_new_transient_produces_a_beat_pulse() {
        let shared = Arc::new(SharedAudio::new());
        let mut analyzer = Analyzer::new(SAMPLE_RATE, Arc::clone(&shared));
        analyzer.push_interleaved(&vec![0.0; SAMPLE_RATE as usize / 20], 1);
        let burst = tone(90.0, 0.15, 0.8);
        analyzer.push_interleaved(&burst, 1);

        assert!(shared.snapshot().beat > 0.1);
    }

    #[test]
    fn shared_snapshot_round_trips_a_complete_frame() {
        let shared = SharedAudio::new();
        let expected = AudioFrame {
            bands: std::array::from_fn(|index| index as f32 / BAND_COUNT as f32),
            energy: 0.42,
            bass: 0.3,
            mid: 0.4,
            treble: 0.5,
            beat: 0.9,
        };
        shared.publish(expected);

        assert_eq!(shared.snapshot(), expected);
    }

    fn analyze_tone(frequency: f32) -> AudioFrame {
        let shared = Arc::new(SharedAudio::new());
        let mut analyzer = Analyzer::new(SAMPLE_RATE, Arc::clone(&shared));
        analyzer.push_interleaved(&tone(frequency, 0.4, 0.35), 1);
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
}
