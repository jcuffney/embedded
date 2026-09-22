//! Portable PCM audio synthesis: tones and simple melodies.
//!
//! "PCM" (pulse-code modulation) is just audio as a stream of numbers: the
//! speaker cone's position sampled 44 100 times per second, each sample a
//! signed 16-bit integer. Stereo streams interleave the two channels
//! `L, R, L, R, …`; one L+R pair is called a *frame*.
//!
//! The design centers on one trait, [`AudioSource`]: "fill this buffer with
//! the next samples". That inversion — the *consumer* calls us when it wants
//! data — is exactly how real audio hardware works (a Bluetooth stack's data
//! callback, an I2S DMA engine refilling a ring buffer), and it's what makes
//! this crate testable on the host: a unit test is just another consumer.

#![no_std]

// The LIBRARY never links std — but the test binary always does (it runs on
// the host, and the test harness itself needs std). This compiles to nothing
// outside `cargo test`.
#[cfg(test)]
extern crate std;

/// The sample rate everything in this crate generates at.
///
/// 44.1 kHz is not arbitrary: it's CD quality, and it's the default the
/// Bluetooth A2DP profile's mandatory SBC codec negotiates. Generating at the
/// rate the consumer expects means no resampling anywhere.
pub const SAMPLE_RATE_HZ: u32 = 44_100;

/// Anything that can fill an interleaved stereo i16 PCM buffer.
///
/// This is the crate's trait seam (the pattern from docs/decisions/0006):
/// board code hands the hardware's raw buffer to a generic `impl
/// AudioSource`, and tests drive the same implementations directly with a
/// plain array. `buf` is `L, R, L, R, …`; if the consumer ever passes an
/// odd-length buffer the trailing lone sample is zeroed, never left as
/// uninitialized garbage.
pub trait AudioSource {
    /// Fill `buf` completely with the next samples, continuing seamlessly
    /// from wherever the previous call left off.
    fn fill(&mut self, buf: &mut [i16]);
}

/// sin(2π·x) for x in turns (one turn = one full period), no `std::sin`.
///
/// `core` has float arithmetic but no trig — `sin()` lives in libm, which
/// bare metal doesn't link. Instead we use Bhāskara I's 7th-century rational
/// approximation: over half a turn (t in 0..=1),
///
/// ```text
/// sin(πt) ≈ 16·t·(1−t) / (5 − 4·t·(1−t))
/// ```
///
/// and get the second half by odd symmetry (sin is the same shape negated).
/// Max absolute error ≈ 0.0016 — about −56 dB, inaudible for tone synthesis.
/// The unit test compares it against `f64::sin` to hold that bound.
fn sine_turns(x: f32) -> f32 {
    // Fold the full turn onto a half turn, remembering the sign.
    let (t, sign) = if x < 0.5 {
        (x * 2.0, 1.0)
    } else {
        ((x - 0.5) * 2.0, -1.0)
    };
    let a = t * (1.0 - t);
    sign * (16.0 * a) / (5.0 - 4.0 * a)
}

/// A pure sine tone at a fixed frequency and amplitude.
///
/// The core trick is a *phase accumulator* (a.k.a. direct digital synthesis,
/// DDS): `phase` is a `u32` where the full 0..=u32::MAX range represents one
/// period of the wave. Each frame we add `step = freq · 2³² / sample_rate`,
/// and integer overflow wraps around exactly like the waveform does — no
/// drift, no division per sample, and because the phase lives in the struct,
/// a tone split across many `fill()` calls of arbitrary sizes is
/// sample-for-sample identical to one big fill. Consumers hand us whatever
/// buffer sizes they like; continuity is automatic.
pub struct Tone {
    phase: u32,
    step: u32,
    amplitude: i16,
}

impl Tone {
    /// `freq_hz = 0` yields silence (the accumulator never moves off zero
    /// and sin(0) = 0), which is what [`Melody`] uses for rests.
    pub fn new(freq_hz: u32, amplitude: i16) -> Self {
        Tone {
            phase: 0,
            // u64 keeps `freq · 2³²` from overflowing before the divide.
            step: ((u64::from(freq_hz) << 32) / u64::from(SAMPLE_RATE_HZ)) as u32,
            amplitude,
        }
    }
}

impl AudioSource for Tone {
    fn fill(&mut self, buf: &mut [i16]) {
        let mut frames = buf.chunks_exact_mut(2);
        for frame in &mut frames {
            // Map the integer phase to turns in [0, 1). f32 only keeps ~24
            // bits of it, but that's a phase error of 2⁻²⁴ turns — far below
            // anything audible.
            let x = self.phase as f32 / 4_294_967_296.0; // / 2³²
            let sample = (sine_turns(x) * f32::from(self.amplitude)) as i16;
            // Mono source, duplicated to both stereo channels.
            frame[0] = sample;
            frame[1] = sample;
            self.phase = self.phase.wrapping_add(self.step);
        }
        for lone in frames.into_remainder() {
            *lone = 0;
        }
    }
}

/// One melody step: a frequency (0 = rest) held for a duration.
pub struct Note {
    pub freq_hz: u32,
    pub ms: u16,
}

/// Frames over which each note fades in/out (~2 ms at 44.1 kHz).
///
/// Why ramp at all: switching frequencies mid-wave creates a discontinuity —
/// the speaker cone is asked to jump instantly, which you hear as a click.
/// A 2 ms linear fade is too short to hear as a fade but long enough to turn
/// the jump into a smooth hand-off. The `melody_has_no_clicks` test pins
/// this down by bounding the sample-to-sample delta.
const RAMP_FRAMES: u32 = SAMPLE_RATE_HZ * 2 / 1000;

/// Plays a sequence of [`Note`]s on repeat, with click-free transitions.
pub struct Melody<'a> {
    notes: &'a [Note],
    idx: usize,
    tone: Tone,
    amplitude: i16,
    note_frames: u32,
    frames_left: u32,
}

impl<'a> Melody<'a> {
    /// Panics if `notes` is empty or any note has `ms == 0` (a zero-length
    /// note could never make progress).
    pub fn new(notes: &'a [Note], amplitude: i16) -> Self {
        assert!(!notes.is_empty(), "melody needs at least one note");
        assert!(
            notes.iter().all(|n| n.ms > 0),
            "zero-length notes make no progress"
        );
        let mut melody = Melody {
            notes,
            idx: 0,
            tone: Tone::new(0, 0),
            amplitude,
            note_frames: 0,
            frames_left: 0,
        };
        melody.load(0);
        melody
    }

    fn load(&mut self, idx: usize) {
        let note = &self.notes[idx];
        // A rest is a tone at amplitude 0, not frequency 0 alone: if the
        // phase accumulator happened to stop mid-wave, a "0 Hz tone" would
        // hold a constant non-zero level (DC) and pop on the next note.
        let amplitude = if note.freq_hz == 0 { 0 } else { self.amplitude };
        // Fresh Tone == phase restarts at zero each note. That IS a phase
        // discontinuity, but the attack ramp below is what makes it silent.
        self.tone = Tone::new(note.freq_hz, amplitude);
        self.note_frames = (u64::from(note.ms) * u64::from(SAMPLE_RATE_HZ) / 1000) as u32;
        self.frames_left = self.note_frames;
        self.idx = idx;
    }
}

impl AudioSource for Melody<'_> {
    fn fill(&mut self, buf: &mut [i16]) {
        let mut frames = buf.chunks_exact_mut(2);
        for frame in &mut frames {
            if self.frames_left == 0 {
                // Wraps around: the melody loops forever, like blink().
                self.load((self.idx + 1) % self.notes.len());
            }
            self.tone.fill(frame);
            // Linear attack × release envelope; both factors saturate at 1.0
            // away from the note edges, so mid-note this multiplies by 1.
            let pos = self.note_frames - self.frames_left;
            let attack = (pos + 1).min(RAMP_FRAMES) as f32 / RAMP_FRAMES as f32;
            let release = self.frames_left.min(RAMP_FRAMES) as f32 / RAMP_FRAMES as f32;
            let envelope = attack * release;
            if envelope < 1.0 {
                for sample in frame.iter_mut() {
                    *sample = (f32::from(*sample) * envelope) as i16;
                }
            }
            self.frames_left -= 1;
        }
        for lone in frames.into_remainder() {
            *lone = 0;
        }
    }
}

/// Adapter for consumers that hand out raw byte buffers (the Bluetooth A2DP
/// data callback does): fill `buf` with 16-bit little-endian PCM.
///
/// Works through a small on-stack chunk — no heap allocation, because the
/// expected caller runs on a real-time audio path where allocating (or
/// anything else slow) causes dropouts. A trailing odd byte is zeroed;
/// never trust a C callback to keep its buffers aligned.
pub fn fill_bytes(src: &mut impl AudioSource, buf: &mut [u8]) {
    let mut rest = buf;
    let mut chunk = [0i16; 64];
    while rest.len() >= 2 {
        let n = chunk.len().min(rest.len() / 2);
        let samples = &mut chunk[..n];
        src.fill(samples);
        let (head, tail) = rest.split_at_mut(n * 2);
        for (sample, out) in samples.iter().zip(head.chunks_exact_mut(2)) {
            out.copy_from_slice(&sample.to_le_bytes());
        }
        rest = tail;
    }
    if let [lone] = rest {
        *lone = 0;
    }
}

// Host-run unit tests (the repo pattern from crates/blink): the library is
// no_std, the test binary links std — which is exactly what lets us judge
// our hand-rolled sine against std's f64::sin. Everything here is
// synchronous math, so unlike blink there's no future-polling or MockDriver.
#[cfg(test)]
mod tests {
    use super::*;

    const AMP: i16 = 8_000;

    /// Render `frames` stereo frames from a source into a Vec.
    fn render(src: &mut impl AudioSource, frames: usize) -> std::vec::Vec<i16> {
        let mut buf = std::vec![0i16; frames * 2];
        src.fill(&mut buf);
        buf
    }

    /// Count sign changes on the left channel — a frequency measurement:
    /// a sine at f Hz crosses zero 2·f times per second.
    fn zero_crossings(samples: &[i16]) -> usize {
        samples
            .chunks_exact(2)
            .map(|frame| frame[0])
            .collect::<std::vec::Vec<_>>()
            .windows(2)
            .filter(|w| (w[0] < 0) != (w[1] < 0))
            .count()
    }

    #[test]
    fn sine_approximation_stays_within_bhaskara_error_bound() {
        let mut max_err = 0.0f64;
        for i in 0..10_000 {
            let x = f64::from(i) / 10_000.0;
            let approx = f64::from(sine_turns(x as f32));
            let exact = (x * 2.0 * core::f64::consts::PI).sin();
            max_err = max_err.max((approx - exact).abs());
        }
        // Bhāskara I's published bound is ~0.00163; leave a little headroom
        // for the f32 arithmetic.
        assert!(max_err < 0.002, "max error {max_err}");
    }

    #[test]
    fn tone_samples_stay_within_amplitude() {
        let buf = render(&mut Tone::new(440, AMP), 44_100);
        assert!(buf.iter().all(|&s| s.abs() <= AMP));
        // ... and actually reach near it (a broken generator emitting
        // near-silence would pass the bound check alone).
        assert!(buf.iter().any(|&s| s.abs() > AMP - AMP / 10));
    }

    #[test]
    fn tone_440hz_crosses_zero_880_times_per_second() {
        let buf = render(&mut Tone::new(440, AMP), 44_100);
        let crossings = zero_crossings(&buf);
        assert!((878..=882).contains(&crossings), "got {crossings}");
    }

    #[test]
    fn phase_continues_seamlessly_across_split_fills() {
        // The property that matters for real playback: the consumer's buffer
        // sizes must not affect the waveform. One 4096-sample fill and two
        // 2048-sample fills must be sample-for-sample identical.
        let whole = render(&mut Tone::new(440, AMP), 2_048);
        let mut split_tone = Tone::new(440, AMP);
        let mut halves = render(&mut split_tone, 1_024);
        halves.extend(render(&mut split_tone, 1_024));
        assert_eq!(whole, halves);
    }

    #[test]
    fn tone_is_mono_duplicated_to_both_channels() {
        let buf = render(&mut Tone::new(440, AMP), 1_000);
        assert!(buf.chunks_exact(2).all(|frame| frame[0] == frame[1]));
    }

    #[test]
    fn tone_has_no_dc_offset_over_whole_periods() {
        // 441 Hz divides 44 100 exactly: 100 frames per period, so summing
        // whole periods should cancel to (nearly) zero. A DC offset would
        // push the speaker cone off-center and distort at high volume.
        let buf = render(&mut Tone::new(441, AMP), 100 * 40);
        let mean = buf.iter().map(|&s| f64::from(s)).sum::<f64>() / buf.len() as f64;
        assert!(mean.abs() < f64::from(AMP) * 0.01, "mean {mean}");
    }

    #[test]
    fn fill_bytes_encodes_little_endian_and_zeroes_odd_tail() {
        // Same tone rendered as i16 and as bytes must agree after decoding.
        let direct = render(&mut Tone::new(440, AMP), 128);
        let mut bytes = std::vec![0u8; 128 * 2 * 2];
        fill_bytes(&mut Tone::new(440, AMP), &mut bytes);
        let decoded: std::vec::Vec<i16> = bytes
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(direct, decoded);

        let mut odd = [0xAAu8; 7];
        fill_bytes(&mut Tone::new(440, AMP), &mut odd);
        assert_eq!(odd[6], 0, "trailing odd byte must be zeroed");
    }

    #[test]
    fn melody_switches_frequency_at_the_note_boundary() {
        let notes = [
            Note {
                freq_hz: 440,
                ms: 100,
            },
            Note {
                freq_hz: 880,
                ms: 100,
            },
        ];
        let frames_per_note = 4_410; // 100 ms at 44.1 kHz
        let buf = render(&mut Melody::new(&notes, AMP), frames_per_note * 2);
        let (first, second) = buf.split_at(frames_per_note * 2);
        // 100 ms of 440 Hz ≈ 88 crossings; of 880 Hz ≈ 176. The envelope
        // never moves a crossing (scaling can't change a sample's sign).
        let a = zero_crossings(first);
        let b = zero_crossings(second);
        assert!((86..=90).contains(&a), "first note: {a}");
        assert!((174..=178).contains(&b), "second note: {b}");
    }

    #[test]
    fn melody_has_no_clicks_at_note_boundaries() {
        // A click is a near-instant jump. The steepest legitimate slope of a
        // sine is amplitude · 2π · f / sample_rate — for 880 Hz at AMP
        // that's ≈ 1 003 per sample. Anything much larger means the ramp
        // failed and adjacent notes are colliding mid-wave.
        let notes = [
            Note {
                freq_hz: 440,
                ms: 50,
            },
            Note {
                freq_hz: 880,
                ms: 50,
            },
            Note {
                freq_hz: 660,
                ms: 50,
            },
        ];
        let buf = render(&mut Melody::new(&notes, AMP), 44_100 / 2);
        let left: std::vec::Vec<i16> = buf.chunks_exact(2).map(|f| f[0]).collect();
        let max_delta = left
            .windows(2)
            .map(|w| (i32::from(w[1]) - i32::from(w[0])).abs())
            .max()
            .unwrap();
        assert!(max_delta <= 1_100, "max sample-to-sample jump {max_delta}");
    }

    #[test]
    fn rest_notes_are_silent_and_melody_loops() {
        let notes = [
            Note { freq_hz: 0, ms: 50 },
            Note {
                freq_hz: 440,
                ms: 50,
            },
        ];
        let frames_per_note = 2_205; // 50 ms
        // Render both notes plus a wrap back into the rest.
        let buf = render(
            &mut Melody::new(&notes, AMP),
            frames_per_note * 2 + frames_per_note / 2,
        );
        let rest = &buf[..frames_per_note * 2];
        assert!(rest.iter().all(|&s| s == 0), "rest must be pure silence");
        let wrapped = &buf[frames_per_note * 4..];
        assert!(
            wrapped.iter().all(|&s| s == 0),
            "after the last note the melody must loop back to the rest"
        );
    }
}
