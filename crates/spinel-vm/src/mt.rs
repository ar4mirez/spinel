//! MT19937, the generator behind `Random` (#249).
//!
//! The reference algorithm (Matsumoto and Nishimura's `init_genrand`,
//! `init_by_array` and `genrand_int32`), which is what CRuby's `Random` runs;
//! `core/random.rb` decides which initialiser a seed gets and turns the
//! 32-bit outputs into Integers and Floats as CRuby does, and both were
//! checked against ruby 4.0.6's sequences for many seeds.
//!
//! The state lives in a BINARY String the `Random` object holds — 624 words
//! and the index, little-endian — so a generator is an ordinary heap value
//! and there is no process-global one.

const N: usize = 624;
const M: usize = 397;

pub struct Mt {
    state: [u32; N],
    index: usize,
}

impl Mt {
    /// `init_genrand(key[0])` when `single`, `init_by_array(key)` otherwise.
    pub fn new(key: &[u32], single: bool) -> Mt {
        if single {
            return Mt::genrand(key.first().copied().unwrap_or(0));
        }
        let mut mt = Mt::genrand(19_650_218);
        let s = &mut mt.state;
        let (mut i, mut j) = (1usize, 0usize);
        let len = key.len().max(1);
        for _ in 0..N.max(len) {
            let prev = s[i - 1] ^ (s[i - 1] >> 30);
            s[i] = (s[i] ^ prev.wrapping_mul(1_664_525))
                .wrapping_add(key.get(j).copied().unwrap_or(0))
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                s[0] = s[N - 1];
                i = 1;
            }
            if j >= len {
                j = 0;
            }
        }
        for _ in 0..N - 1 {
            let prev = s[i - 1] ^ (s[i - 1] >> 30);
            s[i] = (s[i] ^ prev.wrapping_mul(1_566_083_941)).wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                s[0] = s[N - 1];
                i = 1;
            }
        }
        s[0] = 0x8000_0000;
        mt
    }

    fn genrand(seed: u32) -> Mt {
        let mut state = [0u32; N];
        state[0] = seed;
        for i in 1..N {
            state[i] = 1_812_433_253u32
                .wrapping_mul(state[i - 1] ^ (state[i - 1] >> 30))
                .wrapping_add(i as u32);
        }
        Mt { state, index: N }
    }

    pub fn next_u32(&mut self) -> u32 {
        if self.index >= N {
            for k in 0..N {
                let y = (self.state[k] & 0x8000_0000) | (self.state[(k + 1) % N] & 0x7fff_ffff);
                let mag = if y & 1 == 0 { 0 } else { 0x9908_b0df };
                self.state[k] = self.state[(k + M) % N] ^ (y >> 1) ^ mag;
            }
            self.index = 0;
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity((N + 1) * 4);
        for word in self.state {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(&(self.index as u32).to_le_bytes());
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Mt> {
        if bytes.len() != (N + 1) * 4 {
            return None;
        }
        let word =
            |i: usize| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().expect("four bytes"));
        let mut state = [0u32; N];
        for (i, slot) in state.iter_mut().enumerate() {
            *slot = word(i);
        }
        let index = (word(N) as usize).min(N);
        Some(Mt { state, index })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ruby 4.0.6: `Random.new(42).bytes(4).unpack1("V")`.
    #[test]
    fn a_small_seed_is_init_genrand() {
        assert_eq!(Mt::new(&[42], true).next_u32(), 1_608_637_542);
    }

    /// The reference implementation's own check: `init_by_array` of
    /// `{0x123, 0x234, 0x345, 0x456}` starts 1067595299, 955945823.
    #[test]
    fn init_by_array_is_the_reference_one() {
        let mut mt = Mt::new(&[0x123, 0x234, 0x345, 0x456], false);
        assert_eq!(mt.next_u32(), 1_067_595_299);
        assert_eq!(mt.next_u32(), 955_945_823);
    }
}
