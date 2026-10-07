//! Small messages as sound: a speaker on one device, a microphone on the
//! other. See docs/PROTOCOL.md §14.
//!
//! Slow (under twenty bytes per second) but it needs no line of sight, which
//! makes it a way back for feedback codes when the sender's camera cannot see
//! the receiver's screen.
//!
//! It is meant to be pleasant to hear, since people sit next to it: every
//! symbol is two notes of one pentatonic scale, a lower and a higher one,
//! struck like a music box (quick attack, fading away). Any two notes of that
//! scale sound well together, whatever the data. Each note is one of eight,
//! so a symbol carries six bits. A message is a two-symbol preamble, a length
//! byte, the payload and a CRC.

use std::f32::consts::{PI, TAU};

/// Seconds a symbol lasts.
pub const SYMBOL_SECONDS: f32 = 0.04;
/// The notes of each voice, in Hz: C major pentatonic, C5 to E6 for the
/// lower voice and G6 to C8 for the higher one. Neighbors are at least 64 Hz
/// apart, more than twice the resolution of a symbol-long window.
pub const NOTES: [[f32; TONES]; VOICES] = [
    [
        523.25, 587.33, 659.26, 783.99, 880.00, 1046.50, 1174.66, 1318.51,
    ],
    [
        1567.98, 1760.00, 2093.00, 2349.32, 2637.02, 3135.96, 3520.00, 4186.01,
    ],
];
const TONES: usize = 8;
const VOICES: usize = 2;
/// Bits a note carries.
const NOTE_BITS: usize = 3;
/// Longest payload, in bytes.
pub const MAX_PAYLOAD: usize = 255;

/// Notes of the two preamble symbols (lower voice, higher voice): the two
/// voices cross from the ends of their ranges.
const PREAMBLE: [[usize; VOICES]; 2] = [[0, 7], [7, 0]];
/// The decoder looks for a preamble this many times per symbol.
const HOPS_PER_SYMBOL: usize = 4;
/// A note counts as present when it is this much stronger than the average
/// of the other notes of its voice.
const DOMINANCE: f32 = 4.0;
/// Loudness of the two voices; the higher one is kept softer.
const LEVEL: [f32; VOICES] = [0.32, 0.2];

fn symbol_len(sample_rate: f32) -> usize {
    (SYMBOL_SECONDS * sample_rate).round() as usize
}

/// Payload bytes as the notes of each symbol (preamble included).
fn symbols(payload: &[u8]) -> Vec<[usize; VOICES]> {
    let mut bytes = vec![payload.len() as u8];
    bytes.extend_from_slice(payload);
    let check = crc32fast::hash(&bytes) as u16;
    bytes.extend_from_slice(&check.to_le_bytes());
    // Three bits per note, most significant first, padded with zeros.
    let mut notes: Vec<usize> = Vec::new();
    let (mut acc, mut bits) = (0usize, 0);
    for b in bytes {
        acc = acc << 8 | b as usize;
        bits += 8;
        while bits >= NOTE_BITS {
            bits -= NOTE_BITS;
            notes.push(acc >> bits & (TONES - 1));
        }
    }
    if bits > 0 {
        notes.push(acc << (NOTE_BITS - bits) & (TONES - 1));
    }
    while !notes.len().is_multiple_of(VOICES) {
        notes.push(0);
    }
    PREAMBLE
        .into_iter()
        .chain(notes.as_chunks::<VOICES>().0.iter().copied())
        .collect()
}

/// Notes (three bits each) back to bytes; leftover bits are dropped.
fn bytes_of(notes: &[usize]) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0usize, 0);
    for &n in notes {
        acc = (acc << NOTE_BITS | n) & 0xFFFF;
        bits += NOTE_BITS;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// How long a message of `payload_len` bytes sounds, in seconds.
pub fn duration(payload_len: usize) -> f32 {
    let notes = ((payload_len + 3) * 8).div_ceil(NOTE_BITS);
    (2 + notes.div_ceil(VOICES)) as f32 * SYMBOL_SECONDS
}

/// Renders a message as samples in -1..1 (peak about 0.5).
///
/// # Panics
/// If the payload is longer than [`MAX_PAYLOAD`].
pub fn encode(payload: &[u8], sample_rate: f32) -> Vec<f32> {
    assert!(
        payload.len() <= MAX_PAYLOAD,
        "payload too long for a sound message"
    );
    let n = symbol_len(sample_rate);
    // Struck, not switched on: a few milliseconds of attack, then the note
    // fades to about a third, and is taken away gently so nothing clicks.
    let edge = (n / 10).max(1);
    let fade = -(3.0f32.ln()) / n as f32;
    let mut out = Vec::new();
    for notes in symbols(payload) {
        for s in 0..n {
            let t = s as f32 / sample_rate;
            let wave: f32 = notes
                .iter()
                .enumerate()
                .map(|(v, &note)| (TAU * NOTES[v][note] * t).sin() * LEVEL[v])
                .sum();
            let near = s.min(n - 1 - s);
            let ramp = if near < edge {
                0.5 - 0.5 * (PI * near as f32 / edge as f32).cos()
            } else {
                1.0
            };
            out.push(wave * ramp * (fade * s as f32).exp());
        }
    }
    out
}

/// Power of one frequency in a block of samples (Goertzel).
fn power(samples: &[f32], coefficient: f32) -> f32 {
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for &x in samples {
        let s0 = x + coefficient * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - coefficient * s1 * s2
}

/// The strongest note of each voice in a symbol-long block, and how strongly
/// it stands out (the weaker voice's ratio to the rest of its notes).
struct Chord {
    notes: [usize; VOICES],
    clarity: f32,
    energy: f32,
}

/// A message in the middle of being read. Positions are indices into the
/// decoder's buffer.
struct Reading {
    /// Where its preamble starts. Kept so that, should the message turn out
    /// to be damaged (or no message at all), listening resumes right after
    /// this point instead of after everything it seemed to span.
    origin: usize,
    /// Where its next symbol starts.
    next: usize,
    /// The notes read so far, after the preamble.
    notes: Vec<usize>,
}

/// Listens to a stream of samples and returns the messages in it.
pub struct Decoder {
    /// Goertzel coefficients per voice and note.
    coefficients: [[f32; TONES]; VOICES],
    symbol: usize,
    hop: usize,
    buffer: Vec<f32>,
    /// A message being read.
    reading: Option<Reading>,
}

impl Decoder {
    pub fn new(sample_rate: f32) -> Self {
        let symbol = symbol_len(sample_rate);
        Decoder {
            coefficients: NOTES.map(|voice| voice.map(|f| 2.0 * (TAU * f / sample_rate).cos())),
            symbol,
            hop: (symbol / HOPS_PER_SYMBOL).max(1),
            buffer: Vec::new(),
            reading: None,
        }
    }

    fn read(&self, at: usize) -> Chord {
        let block = &self.buffer[at..at + self.symbol];
        let mut out = Chord {
            notes: [0; VOICES],
            clarity: f32::INFINITY,
            energy: 0.0,
        };
        for (v, coefficients) in self.coefficients.iter().enumerate() {
            let powers = coefficients.map(|c| power(block, c));
            let (best, &max) = powers
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .expect("eight notes");
            let rest = (powers.iter().sum::<f32>() - max) / (TONES - 1) as f32;
            out.notes[v] = best;
            out.clarity = out.clarity.min(max / rest.max(1e-12));
            out.energy += max;
        }
        out
    }

    /// How well a preamble starts at `at`: its energy, or None if it is none.
    fn preamble_at(&self, at: usize) -> Option<f32> {
        if at + 2 * self.symbol > self.buffer.len() {
            return None;
        }
        let first = self.read(at);
        if first.notes != PREAMBLE[0] || first.clarity < DOMINANCE {
            return None;
        }
        let second = self.read(at + self.symbol);
        (second.notes == PREAMBLE[1] && second.clarity >= DOMINANCE)
            .then_some(first.energy + second.energy)
    }

    /// Adds samples (-1..1) and returns the messages completed by them.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Vec<u8>> {
        self.buffer.extend_from_slice(samples);
        let mut messages = Vec::new();
        // Where listening for a preamble continues.
        let mut at = 0;
        loop {
            match self.reading.take() {
                None => {
                    // A preamble already matches when a window only catches
                    // its tail or its head, so the first match is rarely the
                    // best aligned: look on for two symbols and take the
                    // loudest.
                    let ahead = 2 * HOPS_PER_SYMBOL;
                    if at + ahead * self.hop + 2 * self.symbol > self.buffer.len() {
                        break;
                    }
                    if self.preamble_at(at).is_none() {
                        at += self.hop;
                        continue;
                    }
                    let (_, best) = (0..=ahead)
                        .filter_map(|h| Some((self.preamble_at(at + h * self.hop)?, h)))
                        .max_by(|a, b| a.0.total_cmp(&b.0))
                        .expect("the first position matches");
                    let origin = at + best * self.hop;
                    self.reading = Some(Reading {
                        origin,
                        next: origin + 2 * self.symbol,
                        notes: Vec::new(),
                    });
                }
                Some(mut r) => {
                    if r.next + self.symbol > self.buffer.len() {
                        at = r.origin;
                        self.reading = Some(r);
                        break;
                    }
                    r.notes.extend(self.read(r.next).notes);
                    r.next += self.symbol;
                    let bytes = bytes_of(&r.notes);
                    match bytes.first().map(|&len| len as usize + 3) {
                        Some(total) if bytes.len() >= total => {
                            let (body, check) = bytes[..total].split_at(total - 2);
                            if (crc32fast::hash(body) as u16).to_le_bytes() == check {
                                messages.push(body[1..].to_vec());
                                at = r.next;
                            } else {
                                // Not a message after all: listen again from
                                // just past where it seemed to start.
                                at = r.origin + self.symbol;
                            }
                        }
                        _ => self.reading = Some(r),
                    }
                }
            }
        }
        // Keep only what is still needed.
        let keep_from = at.min(self.buffer.len());
        self.buffer.drain(..keep_from);
        if let Some(r) = &mut self.reading {
            r.origin -= keep_from;
            r.next -= keep_from;
        }
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(len: usize, seed: u64, amplitude: f32) -> Vec<f32> {
        let mut x = seed | 1;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                ((x >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0) * amplitude
            })
            .collect()
    }

    fn payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 37 + 11) as u8).collect()
    }

    fn decode_all(samples: &[f32], rate: f32) -> Vec<Vec<u8>> {
        // In pieces of odd size, as a microphone delivers them.
        let mut d = Decoder::new(rate);
        samples.chunks(733).flat_map(|c| d.push(c)).collect()
    }

    #[test]
    fn clean_roundtrip_at_common_rates() {
        for rate in [44_100.0, 48_000.0, 16_000.0] {
            for len in [0, 1, 24, 255] {
                let p = payload(len);
                let mut samples = vec![0.0; 1000];
                samples.extend(encode(&p, rate));
                samples.extend(vec![0.0; 4000]);
                assert_eq!(
                    decode_all(&samples, rate),
                    vec![p],
                    "{rate} Hz, {len} bytes"
                );
            }
        }
        // A feedback code of 24 bytes takes a second and a half.
        assert!((duration(24) - 1.52).abs() < 0.01, "{}", duration(24));
    }

    #[test]
    fn several_messages_in_noise_at_any_offset() {
        let rate = 48_000.0;
        let (a, b) = (payload(20), payload(31));
        for offset in [0, 137, 480, 1111, 1919] {
            let mut samples = noise(5000 + offset, 3, 0.05);
            samples.extend(encode(&a, rate));
            samples.extend(noise(9000, 4, 0.05));
            samples.extend(encode(&b, rate));
            samples.extend(noise(6000, 5, 0.05));
            // Quiet reception with noise as loud as a tone.
            let floor = noise(samples.len(), 6, 0.05);
            let heard: Vec<f32> = samples
                .iter()
                .zip(&floor)
                .map(|(s, n)| s * 0.2 + n)
                .collect();
            assert_eq!(
                decode_all(&heard, rate),
                vec![a.clone(), b.clone()],
                "offset {offset}"
            );
        }
    }

    #[test]
    fn survives_echo() {
        let rate = 48_000.0;
        let p = payload(40);
        let direct = encode(&p, rate);
        let mut heard = vec![0.0; direct.len() + 20_000];
        // The direct sound, a strong early reflection and a later, weaker one.
        for (delay, gain) in [(2000, 1.0), (2000 + 700, 0.6), (2000 + 3300, 0.35)] {
            for (i, s) in direct.iter().enumerate() {
                heard[delay + i] += s * gain;
            }
        }
        assert_eq!(decode_all(&heard, rate), vec![p]);
    }

    #[test]
    fn survives_another_sample_rate_and_a_fast_clock() {
        // Played at 44.1 kHz by a device whose clock runs 0.2% fast, heard at 48 kHz.
        let p = payload(60);
        let played = encode(&p, 44_100.0);
        let ratio = 44_100.0 * 1.002 / 48_000.0;
        let len = (played.len() as f32 / ratio) as usize - 1;
        let mut heard = vec![0.0; 3000];
        heard.extend((0..len).map(|i| {
            let x = i as f32 * ratio;
            let (a, f) = (x as usize, x.fract());
            played[a] * (1.0 - f) + played[a + 1] * f
        }));
        heard.extend(vec![0.0; 5000]);
        assert_eq!(decode_all(&heard, 48_000.0), vec![p]);
    }

    #[test]
    fn silence_noise_and_damage_yield_nothing() {
        let rate = 48_000.0;
        assert!(decode_all(&vec![0.0; 50_000], rate).is_empty());
        assert!(decode_all(&noise(100_000, 9, 0.5), rate).is_empty());
        // A message cut off in the middle, then a whole one: only the whole one.
        let (cut, whole) = (encode(&payload(50), rate), payload(10));
        let mut samples = cut[..cut.len() / 2].to_vec();
        samples.extend(vec![0.0; 30_000]);
        samples.extend(encode(&whole, rate));
        samples.extend(vec![0.0; 5000]);
        assert_eq!(decode_all(&samples, rate), vec![whole]);
    }
}
