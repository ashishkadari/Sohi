//! The sounds of full mode: a dark drone that stutters like a torn
//! signal, with scraps of data in it and the cloth of the cape under it.
//! Everything sharp is rounded off: it must be pleasant to hear often.

use std::f32::consts::{PI, TAU};

use super::{Noise, count, filter, rate, seconds};

/// How long a scrap of signal is, in ms.
const SLICE_MS: u32 = 46;
/// Where the highs begin to fall away, in Hz: they are rough on the ears.
const CEILING: f32 = 3_800.0;
/// The pitches of the data, in Hz: they fit the drone.
const DATA: [f32; 6] = [440.0, 523.3, 659.3, 880.0, 1_046.5, 1_318.5];

/// One of the two.
pub(super) struct Glitch {
    /// How long it is, in ms.
    pub(super) ms: u32,
    /// When the screen is covered, in ms: the beat everything goes to.
    pub(super) beat: u32,
    /// Whether the signal builds up, or falls apart.
    pub(super) rising: bool,
}

/// What one slice of the signal is like.
#[derive(Clone, Copy)]
struct Slice {
    /// How loud, 0 to 1.
    gain: f32,
    /// The drone's pitch, as a factor.
    bend: f32,
    /// Every how many samples the signal is renewed.
    hold: usize,
    /// The pitch of a scrap of data; 0 for none.
    data: f32,
}

impl Glitch {
    pub(super) fn wave(&self) -> Vec<f32> {
        let len = count(self.ms);
        let slices = self.slices();
        let width = count(SLICE_MS);
        let mut noise = Noise(0x50_48_49_21);
        let mut drone = Drone::default();
        let (mut held, mut cloth, mut under) = (0.0_f32, 0.0_f32, 0.0_f32);
        let mut round = [0.0_f32; 2];
        (0..len)
            .map(|i| {
                let t = seconds(i);
                let p = t / seconds(len);
                let open = if self.rising { p } else { 1.0 - p };
                let slice = slices.get(i / width).copied().unwrap_or(Slice::CLEAR);
                let within = seconds(i % width);
                // Soft edges: a stutter, not a click.
                let left = seconds(width) - within;
                let edge = (within.min(left) / 0.006).clamp(0.0, 1.0);
                let gain = slice.gain * edge + (1.0 - edge) * 0.35;

                let raw = drone.next(slice.bend, open);
                if i % slice.hold == 0 {
                    held = raw;
                }
                let signal = 0.7 * raw + 0.3 * held;

                cloth += filter(300.0 + 1_500.0 * open) * (noise.next() - cloth);
                under += filter(120.0) * (cloth - under);
                let wind = (cloth - under) * 0.45 * (p * PI).sin().powi(2);

                let blip = if slice.data > 0.0 {
                    let fall = slice.data * (1.0 - 1.5 * within);
                    (TAU * fall * within).sin() * (-within / 0.016).exp() * edge * 0.3
                } else {
                    0.0
                };
                let body = (signal * gain + blip) * swell(p) + wind + self.hit(t);
                // Two low-pass filters in a row, and no hard peaks.
                let mut out = body.tanh();
                for stage in &mut round {
                    *stage += filter(CEILING) * (out - *stage);
                    out = *stage;
                }
                // From and to nothing.
                out * (t / 0.02).min((seconds(len) - t) / 0.08).clamp(0.0, 1.0)
            })
            .collect()
    }

    /// The scraps: the closer to the beat, the calmer when building up;
    /// the further from it, the more torn when falling apart.
    fn slices(&self) -> Vec<Slice> {
        let mut noise = Noise(0x2077_0B1A);
        (0..self.ms.div_ceil(SLICE_MS))
            .map(|n| {
                let at = n * SLICE_MS;
                let settled = at >= self.beat;
                let torn = if self.rising == settled { 0.25 } else { 1.0 };
                let (a, b, c, d) = (noise.next(), noise.next(), noise.next(), noise.next());
                Slice {
                    gain: if a < 0.1 * torn {
                        1.0 - 0.8 * torn
                    } else {
                        1.0
                    },
                    bend: if b > 0.55 { 1.0 + 0.5 * torn } else { 1.0 },
                    hold: if c > 0.2 && torn > 0.5 { 6 } else { 1 },
                    data: if d > 0.8 - 0.8 * torn {
                        pick(noise.next())
                    } else {
                        0.0
                    },
                }
            })
            .collect()
    }

    /// The beat itself: something heavy and soft lands, and rings.
    fn hit(&self, t: f32) -> f32 {
        let after = t - seconds(count(self.beat));
        if after <= 0.0 {
            return 0.0;
        }
        let rise = (after / 0.012).min(1.0);
        let thud = (TAU * (46.0 * after - 1.4 * (-after / 0.05).exp())).sin();
        let ring = [220.0, 329.6, 440.0]
            .iter()
            .map(|pitch| (TAU * pitch * after).sin())
            .sum::<f32>();
        let strength = if self.rising { 1.0 } else { 0.6 };
        strength * rise * (0.36 * thud * (-after / 0.2).exp() + 0.09 * ring * (-after / 0.16).exp())
    }
}

impl Slice {
    const CLEAR: Self = Self {
        gain: 1.0,
        bend: 1.0,
        hold: 1,
        data: 0.0,
    };
}

/// One of the data's pitches.
fn pick(noise: f32) -> f32 {
    let step = 2.0 / 6.0;
    let mut edge = -1.0 + step;
    for pitch in DATA {
        if noise < edge {
            return pitch;
        }
        edge += step;
    }
    DATA[0]
}

/// In over a third of the time, out over the last third.
fn swell(p: f32) -> f32 {
    let x = (p / 0.35).min((1.0 - p) / 0.3).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Two low notes, slightly apart, each with a few overtones.
#[derive(Default)]
struct Drone {
    phases: [f32; 2],
}

impl Drone {
    fn next(&mut self, bend: f32, open: f32) -> f32 {
        let mut sum = 0.0;
        for (phase, pitch) in self.phases.iter_mut().zip([110.0, 110.9]) {
            let pitch = pitch * bend * (0.75 + 0.25 * open);
            *phase = (*phase + pitch / rate()).fract();
            let x = TAU * *phase;
            // Brighter as it opens up.
            sum += 0.6 * x.sin()
                + 0.7 * (2.0 * x).sin()
                + open * (0.6 * (3.0 * x).sin() + 0.35 * (5.0 * x).sin() + 0.2 * (8.0 * x).sin());
        }
        sum * 0.2
    }
}
