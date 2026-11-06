//! `MarsagliaPolarGaussian`, the cached Gaussian generator used by the
//! legacy and xoroshiro random sources.

use crate::RandomSource;

/// Port of `net.minecraft.world.level.levelgen.MarsagliaPolarGaussian`.
#[derive(Debug, Clone, Default)]
pub struct MarsagliaPolarGaussian {
    next_next_gaussian: f64,
    have_next_next_gaussian: bool,
}

impl MarsagliaPolarGaussian {
    pub const fn new() -> Self {
        Self {
            next_next_gaussian: 0.0,
            have_next_next_gaussian: false,
        }
    }

    /// Java: `MarsagliaPolarGaussian.reset()`.
    pub fn reset(&mut self) {
        self.have_next_next_gaussian = false;
    }

    /// Java: `MarsagliaPolarGaussian.nextGaussian()`.
    pub fn next(&mut self, random: &mut dyn RandomSource) -> f64 {
        if self.have_next_next_gaussian {
            self.have_next_next_gaussian = false;
            return self.next_next_gaussian;
        }

        let mut d;
        let mut d1;
        let mut d2;
        loop {
            d = 2.0 * random.next_double() - 1.0;
            d1 = 2.0 * random.next_double() - 1.0;
            d2 = d * d + d1 * d1;
            if d2 < 1.0 && d2 != 0.0 {
                break;
            }
        }

        let square_root = (-2.0 * d2.ln() / d2).sqrt();
        self.next_next_gaussian = d1 * square_root;
        self.have_next_next_gaussian = true;
        d * square_root
    }
}
