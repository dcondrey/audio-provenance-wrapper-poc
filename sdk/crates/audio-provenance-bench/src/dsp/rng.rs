/// PCG-XSH-RR 64/32. Hand-written rather than pulled from `rand` because the bench's whole
/// reproducibility claim is that a (track, channel, seed) triple replays a row exactly, and `rand`
/// explicitly does not promise value stability across releases.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
    inc: u64,
    spare_gaussian: Option<f32>,
}

const MULTIPLIER: u64 = 6_364_136_223_846_793_005;

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut rng = Self {
            state: 0,
            inc: (seed << 1) | 1,
            spare_gaussian: None,
        };
        let _ = rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        let _ = rng.next_u32();
        rng
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(MULTIPLIER).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    pub fn next_unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    pub fn next_symmetric(&mut self) -> f32 {
        self.next_unit().mul_add(2.0, -1.0)
    }

    pub fn next_gaussian(&mut self) -> f32 {
        if let Some(spare) = self.spare_gaussian.take() {
            return spare;
        }
        let u1 = self.next_unit().max(f32::MIN_POSITIVE);
        let u2 = self.next_unit();
        let radius = (-2.0 * u1.ln()).sqrt();
        let theta = core::f32::consts::TAU * u2;
        self.spare_gaussian = Some(radius * theta.sin());
        radius * theta.cos()
    }
}
