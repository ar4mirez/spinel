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
//! and there is no process-global one. A draw runs on those bytes in place.

const N: usize = 624;
const M: usize = 397;
/// A state's length in bytes: the 624 words and the index.
pub const STATE_LEN: usize = (N + 1) * 4;

/// A generator, run where its state lies: a view of the state String's
/// bytes, read and written word by word, so a draw touches the words it
/// uses rather than copying all 2,500 bytes in and out (#703).
pub struct Mt<'a> {
    bytes: &'a mut [u8],
}

impl<'a> Mt<'a> {
    /// `init_genrand(key[0])` when `single`, `init_by_array(key)` otherwise:
    /// a fresh state, as the bytes a `Random` holds.
    pub fn seed(key: &[u32], single: bool) -> Vec<u8> {
        let s = if single {
            genrand(key.first().copied().unwrap_or(0))
        } else {
            by_array(key)
        };
        let mut out = Vec::with_capacity(STATE_LEN);
        for word in s {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out.extend_from_slice(&(N as u32).to_le_bytes());
        out
    }

    /// A Fixnum seed's key: its absolute value as 32-bit words, least
    /// significant first, and whether `init_genrand` takes it — one word —
    /// or `init_by_array`, with a top word of 1 dropped first, as
    /// `Random.__state_for__` does for any seed (#705).
    pub fn seed_words(seed: i64) -> (Vec<u32>, bool) {
        let n = seed.unsigned_abs();
        let (low, high) = (n as u32, (n >> 32) as u32);
        match high {
            0 => (vec![low], true),
            1 => (vec![low], false),
            _ => (vec![low, high], false),
        }
    }

    /// The generator whose state is `bytes`, or `None` when they are not one.
    pub fn from_bytes(bytes: &'a mut [u8]) -> Option<Mt<'a>> {
        (bytes.len() == STATE_LEN).then_some(Mt { bytes })
    }

    fn word(&self, i: usize) -> u32 {
        u32::from_le_bytes(self.bytes[i * 4..i * 4 + 4].try_into().expect("four bytes"))
    }

    fn set_word(&mut self, i: usize, word: u32) {
        self.bytes[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut index = (self.word(N) as usize).min(N);
        if index >= N {
            for k in 0..N {
                let y = (self.word(k) & 0x8000_0000) | (self.word((k + 1) % N) & 0x7fff_ffff);
                let mag = if y & 1 == 0 { 0 } else { 0x9908_b0df };
                let next = self.word((k + M) % N) ^ (y >> 1) ^ mag;
                self.set_word(k, next);
            }
            index = 0;
        }
        let mut y = self.word(index);
        self.set_word(N, (index + 1) as u32);
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// `size` bytes from `ceil(size / 4)` outputs, each least significant
    /// byte first and the last cut short: CRuby's `rand_mt_get_bytes`, what
    /// `Random#bytes` answers (#744).
    pub fn bytes(&mut self, size: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(size.next_multiple_of(4));
        while out.len() < size {
            out.extend_from_slice(&self.next_u32().to_le_bytes());
        }
        out.truncate(size);
        out
    }

    /// `genrand_res53`: a Float in [0, 1) from two outputs, or in [0, 1]
    /// when `inclusive` — their 64 bits scaled by 2**53 + 1, as CRuby's
    /// `int_pair_to_real_inclusive` does (#705).
    pub fn real(&mut self, inclusive: bool) -> f64 {
        let a = self.next_u32();
        let b = self.next_u32();
        if inclusive {
            let bits = (u128::from(a) << 32) | u128::from(b);
            ((bits * ((1u128 << 53) + 1)) >> 64) as f64 / 9_007_199_254_740_992.0
        } else {
            (f64::from(a >> 5) * 67_108_864.0 + f64::from(b >> 6)) / 9_007_199_254_740_992.0
        }
    }

    /// A uniform value in 0..=limit: CRuby's `limited_rand` for a limit that
    /// fits 32 bits. Every bit below the top one of `limit` is the mask, and
    /// an output past `limit` under it is drawn again.
    pub fn limited(&mut self, limit: u32) -> u32 {
        if limit == 0 {
            return 0;
        }
        let mask = u32::MAX >> limit.leading_zeros();
        loop {
            let value = self.next_u32() & mask;
            if value <= limit {
                return value;
            }
        }
    }
}

fn genrand(seed: u32) -> [u32; N] {
    let mut state = [0u32; N];
    state[0] = seed;
    for i in 1..N {
        state[i] = 1_812_433_253u32
            .wrapping_mul(state[i - 1] ^ (state[i - 1] >> 30))
            .wrapping_add(i as u32);
    }
    state
}

fn by_array(key: &[u32]) -> [u32; N] {
    let mut s = genrand(19_650_218);
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
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ruby 4.0.6: `Random.new(42).bytes(4).unpack1("V")`.
    #[test]
    fn a_small_seed_is_init_genrand() {
        let mut state = Mt::seed(&[42], true);
        assert_eq!(
            Mt::from_bytes(&mut state).unwrap().next_u32(),
            1_608_637_542
        );
    }

    /// The reference implementation's own check: `init_by_array` of
    /// `{0x123, 0x234, 0x345, 0x456}` starts 1067595299, 955945823.
    #[test]
    fn init_by_array_is_the_reference_one() {
        let mut state = Mt::seed(&[0x123, 0x234, 0x345, 0x456], false);
        let mut mt = Mt::from_bytes(&mut state).unwrap();
        assert_eq!(mt.next_u32(), 1_067_595_299);
        assert_eq!(mt.next_u32(), 955_945_823);
    }

    /// Ruby 4.0.6: `r = Random.new(42); 5.times.map { r.rand(6) }`.
    #[test]
    fn limited_draws_as_limited_rand() {
        let mut state = Mt::seed(&[42], true);
        let mut mt = Mt::from_bytes(&mut state).unwrap();
        let draws: Vec<u32> = (0..5).map(|_| mt.limited(5)).collect();
        assert_eq!(draws, [3, 4, 2, 4, 4]);
    }

    /// Ruby 4.0.9: `r = Random.new(42); [r.rand, r.rand(1.0..2.0) - 1]`,
    /// the second through the inclusive scaling.
    #[test]
    fn real_draws_as_genrand_res53() {
        let mut state = Mt::seed(&[42], true);
        let mut mt = Mt::from_bytes(&mut state).unwrap();
        assert_eq!(mt.real(false), 0.3745401188473625);
        assert_eq!(mt.real(true) + 1.0, 1.950714311605188);
    }

    /// Ruby 4.0.9: `r = Random.new(42); [r.bytes(7).bytes, r.rand(100)]` —
    /// a word a four bytes, the last cut short and its rest not drawn again.
    #[test]
    fn bytes_are_the_words_least_significant_first() {
        let mut state = Mt::seed(&[42], true);
        let mut mt = Mt::from_bytes(&mut state).unwrap();
        assert_eq!(mt.bytes(7), [102, 220, 225, 95, 179, 61, 234]);
        assert_eq!(mt.bytes(0), []);
        let mut state = Mt::seed(&[42], true);
        let mut mt = Mt::from_bytes(&mut state).unwrap();
        mt.bytes(3);
        assert_eq!(mt.limited(99), 51);
    }

    /// A seed splits as `Random.__state_for__` splits it: one word, a top
    /// word of 1 dropped (2**32 + 5), and two words.
    #[test]
    fn a_seed_splits_into_its_key() {
        assert_eq!(Mt::seed_words(-7), (vec![7], true));
        assert_eq!(Mt::seed_words((1 << 32) + 5), (vec![5], false));
        assert_eq!(Mt::seed_words(1 << 40), (vec![0, 256], false));
    }

    /// A draw leaves the state where it was taken: a second view of the same
    /// bytes carries on the sequence rather than restarting it.
    #[test]
    fn a_draw_advances_the_bytes_in_place() {
        let mut state = Mt::seed(&[42], true);
        let first = Mt::from_bytes(&mut state).unwrap().next_u32();
        let second = Mt::from_bytes(&mut state).unwrap().next_u32();
        let mut fresh = Mt::seed(&[42], true);
        let mut mt = Mt::from_bytes(&mut fresh).unwrap();
        assert_eq!([first, second], [mt.next_u32(), mt.next_u32()]);
    }
}
