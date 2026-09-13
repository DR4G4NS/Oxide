//! Arc `arc.math.Rand` xorshift128+ (audit LOW: seeded sequences).

#[derive(Clone, Debug)]
pub struct ArcRand {
    seed0: u64,
    seed1: u64,
}

impl ArcRand {
    pub fn new(seed: i64) -> Self {
        let mut rand = Self { seed0: 0, seed1: 0 };
        rand.set_seed(seed);
        rand
    }

    /// `Rand.setSeed` / splitmix64-style mix used by Arc.
    pub fn set_seed(&mut self, seed: i64) {
        let mut z = seed as u64 ^ 0x9E37_79B9_7F4A_7C15;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        self.seed0 = z ^ (z >> 31);
        z = self.seed0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        self.seed1 = z ^ (z >> 31);
        if self.seed0 == 0 && self.seed1 == 0 {
            self.seed1 = 0x9E37_79B9_7F4A_7C15;
        }
    }

    /// `Rand.nextLong` xorshift128+.
    pub fn next_long(&mut self) -> i64 {
        let mut s1 = self.seed0;
        let s0 = self.seed1;
        self.seed0 = s0;
        s1 ^= s1 << 23;
        self.seed1 = s1 ^ s0 ^ (s1 >> 17) ^ (s0 >> 26);
        (self.seed1.wrapping_add(s0)) as i64
    }

    pub fn next_float(&mut self) -> f32 {
        ((self.next_long() as u64 >> 40) as f32) * (1.0 / ((1u64 << 24) as f32))
    }

    /// Live xorshift128+ state (NetworkIO world-stream `seed0`/`seed1`).
    pub fn from_state(seed0: u64, seed1: u64) -> Self {
        let mut rand = Self { seed0, seed1 };
        if rand.seed0 == 0 && rand.seed1 == 0 {
            rand.seed1 = 0x9E37_79B9_7F4A_7C15;
        }
        rand
    }

    pub fn state(&self) -> (u64, u64) {
        (self.seed0, self.seed1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_sequence_is_deterministic() {
        let mut a = ArcRand::new(123);
        let mut b = ArcRand::new(123);
        assert_eq!(a.next_long(), b.next_long());
        assert_eq!(a.next_float(), b.next_float());
        let mut c = ArcRand::new(124);
        assert_ne!(ArcRand::new(123).next_long(), c.next_long());
    }
}
