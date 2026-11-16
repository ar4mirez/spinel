//! `DidYouMean::SpellChecker`, as Ruby 4.0's `did_you_mean` has it: the
//! suggestions Bundler's `gem_not_found_message` makes for a gem name it
//! cannot find (`spinel update`, #87).

/// `DidYouMean::SpellChecker.new(dictionary:).correct(input)`.
pub(crate) fn correct<'a>(dictionary: &'a [String], input: &str) -> Vec<&'a str> {
    let normalized_input = normalize(input);
    let input_length = normalized_input.chars().count();
    let threshold = if input_length > 3 { 0.834 } else { 0.77 };

    let mut words: Vec<&str> = dictionary
        .iter()
        .map(String::as_str)
        .filter(|word| jaro_winkler(&normalize(word), &normalized_input) >= threshold)
        .filter(|word| *word != input)
        .collect();
    // `sort_by!` then `reverse!`: the nearest first.
    words.sort_by(|a, b| {
        jaro_winkler(a, &normalized_input).total_cmp(&jaro_winkler(b, &normalized_input))
    });
    words.reverse();

    // Mistypes.
    let threshold = (input_length as f64 * 0.25).ceil() as usize;
    let corrections: Vec<&str> = words
        .iter()
        .copied()
        .filter(|word| levenshtein(&normalize(word), &normalized_input) <= threshold)
        .collect();
    if !corrections.is_empty() {
        return corrections;
    }
    // Misspellings: the first within fewer edits than the shorter's length.
    words
        .into_iter()
        .filter(|word| {
            let word = normalize(word);
            let length = input_length.min(word.chars().count());
            levenshtein(&word, &normalized_input) < length
        })
        .take(1)
        .collect()
}

/// `SpellChecker#normalize`: downcased, without `@`.
fn normalize(word: &str) -> String {
    word.to_lowercase().replace('@', "")
}

/// `DidYouMean::Jaro.distance`.
fn jaro(a: &str, b: &str) -> f64 {
    let (mut s1, mut s2): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if s1.len() > s2.len() {
        std::mem::swap(&mut s1, &mut s2);
    }
    let (length1, length2) = (s1.len(), s2.len());
    let range = if length2 > 3 { length2 / 2 - 1 } else { 0 };
    let mut flags1 = vec![false; length1];
    let mut flags2 = vec![false; length2];
    let mut m = 0.0;
    for i in 0..length1 {
        let last = i + range;
        let mut j = i.saturating_sub(range);
        while j <= last && j < length2 {
            if !flags2[j] && s1[i] == s2[j] {
                flags2[j] = true;
                flags1[i] = true;
                m += 1.0;
                break;
            }
            j += 1;
        }
    }
    let mut t = 0.0;
    let mut k = 0;
    for i in 0..length1 {
        if flags1[i] {
            let mut j = k;
            let mut index = k;
            while j < length2 {
                index = j;
                if flags2[j] {
                    k = j + 1;
                    break;
                }
                j += 1;
            }
            if s2.get(index) != Some(&s1[i]) {
                t += 1.0;
            }
        }
    }
    let t = (t / 2.0_f64).floor();
    if m == 0.0 {
        0.0
    } else {
        (m / length1 as f64 + m / length2 as f64 + (m - t) / m) / 3.0
    }
}

/// `DidYouMean::JaroWinkler.distance`: Jaro, with a bonus for a common
/// prefix of up to four characters when it is over 0.7.
fn jaro_winkler(a: &str, b: &str) -> f64 {
    let distance = jaro(a, b);
    if distance <= 0.7 {
        return distance;
    }
    let prefix = a
        .chars()
        .zip(b.chars())
        .take(4)
        .take_while(|(x, y)| x == y)
        .count();
    distance + prefix as f64 * 0.1 * (1.0 - distance)
}

/// `DidYouMean::Levenshtein.distance`.
fn levenshtein(a: &str, b: &str) -> usize {
    let s1: Vec<char> = a.chars().collect();
    let s2: Vec<char> = b.chars().collect();
    let (n, m) = (s1.len(), s2.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let min3 = |a: usize, b: usize, c: usize| {
        if a < b && a < c {
            a
        } else if b < c {
            b
        } else {
            c
        }
    };
    let mut d: Vec<usize> = (0..=m).collect();
    let mut x = 0;
    for (row, c1) in s1.iter().enumerate() {
        let mut i = row + 1;
        for j in 0..m {
            let cost = usize::from(*c1 != s2[j]);
            x = min3(d[j + 1] + 1, i + 1, d[j] + cost);
            d[j] = i;
            i = x;
        }
        d[m] = x;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dictionary(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    // Each expectation is what Ruby 4.0.9's
    // `DidYouMean::SpellChecker.new(dictionary:).correct` returns.
    #[test]
    fn suggests_as_did_you_mean_does() {
        let gems = dictionary(&[
            "rack",
            "rack-protection",
            "rack-session",
            "rackup",
            "rake",
            "sinatra",
            "tilt",
            "mustermann",
        ]);
        assert_eq!(correct(&gems, "rakc"), ["rake"]);
        assert_eq!(correct(&gems, "sinatr"), ["sinatra"]);
        assert_eq!(correct(&gems, "rack-sesion"), ["rack-session"]);
        assert_eq!(correct(&gems, "mustermen"), ["mustermann"]);
        assert!(correct(&gems, "nokogiri").is_empty());
        assert_eq!(correct(&gems, "rake"), ["rack"]);
    }

    #[test]
    fn distances_match_did_you_mean() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert!((jaro("martha", "marhta") - 0.944_444).abs() < 1e-5);
        assert!((jaro_winkler("martha", "marhta") - 0.961_111).abs() < 1e-5);
        assert_eq!(jaro("abc", "xyz"), 0.0);
    }
}
