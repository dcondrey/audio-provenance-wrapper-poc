use crate::audio;
use crate::error::CodecError;
use crate::watermark::{Detection, WatermarkCodec, bits_of, bytes_of};
use audio_provenance_audio::AudioBuffer;

const SYNC: u16 = 0xA5C3;
const SYNC_BITS: usize = 16;
const CRC_BITS: usize = 16;

fn crc16_ccitt(bytes: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &byte in bytes {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn quantise(sample: f32) -> i32 {
    (f64::from(sample) * 32_768.0)
        .round()
        .clamp(-32_768.0, 32_767.0) as i32
}

fn dequantise(value: i32) -> f32 {
    value as f32 / 32_768.0
}

/// BENCH FIXTURE, NOT A PRODUCT WATERMARK.
///
/// Payload bits ride in the least significant bit of the 16-bit quantisation of channel 0. It is
/// bit-exact through a bit-transparent path and dies the moment a sample value changes at all, which
/// is precisely why it is here: a bench that scores this as robust is broken.
#[derive(Debug, Clone)]
pub struct Lsb16Fixture {
    payload_len: usize,
}

impl Lsb16Fixture {
    pub const fn new(payload_len: usize) -> Self {
        Self { payload_len }
    }

    const fn frame_bits(&self) -> usize {
        SYNC_BITS + self.payload_len * 8 + CRC_BITS
    }

    fn code_word(&self, payload: &[u8]) -> Vec<u8> {
        let mut bits = bits_of(&SYNC.to_be_bytes());
        bits.extend(bits_of(payload));
        bits.extend(bits_of(&crc16_ccitt(payload).to_be_bytes()));
        bits
    }
}

impl WatermarkCodec for Lsb16Fixture {
    fn name(&self) -> &str {
        "fixture_lsb16"
    }

    fn payload_len(&self) -> usize {
        self.payload_len
    }

    fn is_bench_fixture(&self) -> bool {
        true
    }

    fn describe(&self) -> String {
        format!(
            "BENCH FIXTURE. {} payload bytes plus a 16-bit sync word and CRC-16/CCITT, carried in \
             the least significant bit of the 16-bit quantisation of channel 0 and repeated to fill \
             the file. Expected to survive a bit-transparent path and nothing else.",
            self.payload_len
        )
    }

    fn embed(&self, audio: &AudioBuffer, payload: &[u8]) -> Result<AudioBuffer, CodecError> {
        if payload.len() != self.payload_len {
            return Err(CodecError::PayloadLength {
                found: payload.len(),
                expected: self.payload_len,
            });
        }
        let word = self.code_word(payload);
        if audio.frames() < word.len() {
            return Err(CodecError::TooShort {
                frames: audio.frames(),
                needed: word.len(),
            });
        }
        let mut marked = audio.clone();
        let carrier = marked.channel_mut(0).ok_or(CodecError::TooShort {
            frames: 0,
            needed: 1,
        })?;
        for (frame, slot) in carrier.iter_mut().enumerate() {
            let bit = i32::from(word[frame % word.len()]);
            *slot = dequantise((quantise(*slot) & !1) | bit);
        }
        Ok(audio::admit(marked)?)
    }

    fn detect(&self, audio: &AudioBuffer) -> Result<Detection, CodecError> {
        let word_len = self.frame_bits();
        if audio.frames() < word_len {
            return Ok(Detection::none());
        }
        let Some(carrier) = audio.channel(0) else {
            return Ok(Detection::none());
        };
        let recovered: Vec<u8> = carrier
            .iter()
            .map(|&sample| (quantise(sample) & 1) as u8)
            .collect();

        // The embedder repeats with period `word_len`, so any leading crop only rotates the phase.
        // Searching one period is therefore exhaustive rather than a heuristic.
        let sync_bits = bits_of(&SYNC.to_be_bytes());
        let mut candidates = 0u64;
        for phase in 0..word_len {
            let repeats = (recovered.len() - phase) / word_len;
            if repeats == 0 {
                continue;
            }
            candidates += 1;
            let mut votes = vec![0i32; word_len];
            for repeat in 0..repeats {
                let base = phase + repeat * word_len;
                for (position, vote) in votes.iter_mut().enumerate() {
                    let bit = recovered.get(base + position).copied().unwrap_or(0);
                    *vote += if bit == 1 { 1 } else { -1 };
                }
            }
            let decided: Vec<u8> = votes.iter().map(|&v| u8::from(v > 0)).collect();
            if decided.get(..SYNC_BITS) != Some(sync_bits.as_slice()) {
                continue;
            }
            let payload_end = SYNC_BITS + self.payload_len * 8;
            let Some(payload_bits) = decided.get(SYNC_BITS..payload_end) else {
                continue;
            };
            let Some(crc_bits) = decided.get(payload_end..) else {
                continue;
            };
            let payload = bytes_of(payload_bits);
            let observed = bytes_of(crc_bits);
            if observed.len() != 2 || crc16_ccitt(&payload).to_be_bytes() != observed[..] {
                continue;
            }
            let agreement: i32 = votes.iter().map(|v| v.abs()).sum();
            let confidence = f64::from(agreement) / (repeats * word_len) as f64;
            let corrected: u32 = votes
                .iter()
                .map(|&v| u32::from(v.unsigned_abs() as usize != repeats))
                .sum();
            return Ok(Detection {
                payload: Some(payload),
                confidence,
                bits_corrected: corrected,
                candidates_examined: candidates,
                notes: Some(format!("phase {phase}, {repeats} repeats")),
            });
        }
        Ok(Detection {
            payload: None,
            confidence: 0.0,
            bits_corrected: 0,
            candidates_examined: candidates,
            notes: None,
        })
    }
}

/// BENCH FIXTURE. Embeds nothing and always reports the payload it was constructed with.
///
/// This is the harness's own self-test: against it the false-positive arm must read 1.0 and every
/// exact-recovery cell must read 1.0 for the wrong reason. A bench that cannot make this
/// implementation fail has been executed, not validated.
#[derive(Debug, Clone)]
pub struct AlwaysAcceptFixture {
    payload: Vec<u8>,
}

impl AlwaysAcceptFixture {
    pub fn new(payload: Vec<u8>) -> Self {
        Self { payload }
    }
}

impl WatermarkCodec for AlwaysAcceptFixture {
    fn name(&self) -> &str {
        "fixture_always_accept"
    }

    fn payload_len(&self) -> usize {
        self.payload.len()
    }

    fn is_bench_fixture(&self) -> bool {
        true
    }

    fn describe(&self) -> String {
        "BENCH FIXTURE. Embeds nothing and returns a fixed payload for any input, marked or not. \
         Present so the false-positive arm is proved able to fire."
            .to_owned()
    }

    fn embed(&self, audio: &AudioBuffer, payload: &[u8]) -> Result<AudioBuffer, CodecError> {
        if payload.len() != self.payload.len() {
            return Err(CodecError::PayloadLength {
                found: payload.len(),
                expected: self.payload.len(),
            });
        }
        Ok(audio.clone())
    }

    fn detect(&self, _audio: &AudioBuffer) -> Result<Detection, CodecError> {
        Ok(Detection {
            payload: Some(self.payload.clone()),
            confidence: 1.0,
            bits_corrected: 0,
            candidates_examined: 1,
            notes: Some("fixed payload, no analysis performed".to_owned()),
        })
    }
}

/// BENCH FIXTURE. Embeds nothing and never reports a payload: the opposite pole of the self-test.
#[derive(Debug, Clone)]
pub struct SilentFixture {
    payload_len: usize,
}

impl SilentFixture {
    pub const fn new(payload_len: usize) -> Self {
        Self { payload_len }
    }
}

impl WatermarkCodec for SilentFixture {
    fn name(&self) -> &str {
        "fixture_silent"
    }

    fn payload_len(&self) -> usize {
        self.payload_len
    }

    fn is_bench_fixture(&self) -> bool {
        true
    }

    fn describe(&self) -> String {
        "BENCH FIXTURE. Embeds nothing and never reports a payload. Every recovery cell must read \
         0.0 and the false-positive arm must read 0.0."
            .to_owned()
    }

    fn embed(&self, audio: &AudioBuffer, payload: &[u8]) -> Result<AudioBuffer, CodecError> {
        if payload.len() != self.payload_len {
            return Err(CodecError::PayloadLength {
                found: payload.len(),
                expected: self.payload_len,
            });
        }
        Ok(audio.clone())
    }

    fn detect(&self, _audio: &AudioBuffer) -> Result<Detection, CodecError> {
        Ok(Detection::none())
    }
}
