//! Real-time noise generators. Pure DSP code: no I/O, no allocation per sample.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoiseKind {
    White,
    Pink,
    Brown,
}

impl NoiseKind {
    pub const ALL: [NoiseKind; 3] = [NoiseKind::White, NoiseKind::Pink, NoiseKind::Brown];

    pub fn name(self) -> &'static str {
        match self {
            NoiseKind::White => "white",
            NoiseKind::Pink => "pink",
            NoiseKind::Brown => "brown",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            NoiseKind::White => "White Noise",
            NoiseKind::Pink => "Pink Noise",
            NoiseKind::Brown => "Brown Noise",
        }
    }

    pub fn next(self) -> NoiseKind {
        match self {
            NoiseKind::White => NoiseKind::Pink,
            NoiseKind::Pink => NoiseKind::Brown,
            NoiseKind::Brown => NoiseKind::White,
        }
    }

    pub fn parse(s: &str) -> Option<NoiseKind> {
        let s = s.trim();
        NoiseKind::ALL
            .into_iter()
            .find(|k| k.name().eq_ignore_ascii_case(s))
    }

    pub fn to_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(v: u8) -> NoiseKind {
        match v {
            1 => NoiseKind::Pink,
            2 => NoiseKind::Brown,
            _ => NoiseKind::White,
        }
    }
}

impl fmt::Display for NoiseKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// xorshift64* PRNG: tiny, fast and statistically more than good enough for audio noise.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        // Avoid the all-zero state, which xorshift can never leave.
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15 | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform sample in [-1.0, 1.0).
    pub fn uniform(&mut self) -> f32 {
        let bits = (self.next_u64() >> 40) as u32; // top 24 bits
        bits as f32 * (2.0 / (1u32 << 24) as f32) - 1.0
    }
}

// Output scales chosen so every noise colour has a similar loudness (RMS ~0.17, leaving headroom below 1.0),
// which keeps volume steady when the user switches noise types.
const WHITE_SCALE: f32 = 0.294;
const PINK_SCALE: f32 = 0.0972;
const BROWN_SCALE: f32 = 2.94;

pub struct Generator {
    kind: NoiseKind,
    rng: Rng,
    pink: [f32; 7],
    brown: f32,
}

impl Generator {
    pub fn new(kind: NoiseKind, seed: u64) -> Generator {
        Generator {
            kind,
            rng: Rng::new(seed),
            pink: [0.0; 7],
            brown: 0.0,
        }
    }

    pub fn set_kind(&mut self, kind: NoiseKind) {
        self.kind = kind;
        self.pink = [0.0; 7];
        self.brown = 0.0;
    }

    pub fn next_sample(&mut self) -> f32 {
        let w = self.rng.uniform();
        let out = match self.kind {
            NoiseKind::White => w * WHITE_SCALE,
            NoiseKind::Pink => {
                // Paul Kellett's refined pink filter (-3 dB/octave, accurate above ~10 Hz).
                let b = &mut self.pink;
                b[0] = 0.99886 * b[0] + w * 0.055_517_9;
                b[1] = 0.99332 * b[1] + w * 0.075_075_9;
                b[2] = 0.96900 * b[2] + w * 0.153_852;
                b[3] = 0.86650 * b[3] + w * 0.310_485_6;
                b[4] = 0.55000 * b[4] + w * 0.532_952_2;
                b[5] = -0.7616 * b[5] - w * 0.016_898;
                let pink = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
                b[6] = w * 0.115_926;
                pink * PINK_SCALE
            }
            NoiseKind::Brown => {
                // Leaky integrator (-6 dB/octave); the leak keeps it from drifting.
                self.brown = (self.brown + 0.02 * w) / 1.02;
                self.brown * BROWN_SCALE
            }
        };
        out.clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(kind: NoiseKind) -> (f32, f32, f32) {
        let mut g = Generator::new(kind, 42);
        let n = 4_800_000; // 100 s at 48 kHz
        let (mut sum, mut sq, mut peak) = (0.0f64, 0.0f64, 0.0f32);
        for _ in 0..n {
            let s = g.next_sample();
            assert!(s.is_finite());
            sum += s as f64;
            sq += (s as f64) * (s as f64);
            peak = peak.max(s.abs());
        }
        let mean = (sum / n as f64) as f32;
        let rms = (sq / n as f64).sqrt() as f32;
        (mean, rms, peak)
    }

    #[test]
    fn all_noises_are_bounded_centered_and_similarly_loud() {
        for kind in NoiseKind::ALL {
            let (mean, rms, peak) = stats(kind);
            assert!(peak < 1.0, "{kind}: peak {peak} (would clip)");
            assert!(mean.abs() < 0.05, "{kind}: mean {mean}");
            assert!((0.15..0.19).contains(&rms), "{kind}: rms {rms}");
        }
    }

    #[test]
    fn rng_is_uniform_in_range() {
        let mut r = Rng::new(7);
        let (mut lo, mut hi) = (0i32, 0i32);
        for _ in 0..100_000 {
            let v = r.uniform();
            assert!((-1.0..1.0).contains(&v));
            if v < 0.0 { lo += 1 } else { hi += 1 }
        }
        assert!((lo - hi).abs() < 2_000);
    }

    #[test]
    fn parse_and_cycle() {
        assert_eq!(NoiseKind::parse("PINK"), Some(NoiseKind::Pink));
        assert_eq!(NoiseKind::parse(" brown "), Some(NoiseKind::Brown));
        assert_eq!(NoiseKind::parse("blue"), None);
        let mut k = NoiseKind::White;
        for _ in 0..3 {
            assert_eq!(NoiseKind::from_u8(k.to_u8()), k);
            k = k.next();
        }
        assert_eq!(k, NoiseKind::White);
    }
}
