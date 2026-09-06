use fake::Fake as _;
use fake::faker::lorem::en::Word;
use rand::SeedableRng as _;
use rand_chacha::ChaCha8Rng;
use sha2::{Digest as _, Sha256};

/// Non-alphanumeric characters `OpenObserve` accepts as the "special character" class.
///
/// Deliberately narrow: every one of these survives a shell double-quoted string, a compose `environment:`
/// value and a Docker `-e` argument without escaping, which is where the generated deployments put them.
/// `#` and `=` are left out even though `OpenObserve` accepts them, because a password carrying either stops
/// round-tripping through an env file, where `#` opens a comment and `=` splits the assignment.
const SPECIAL_CHARS: [char; 6] = ['!', '%', '+', '-', '.', '_'];

/// Derive the RNG seed for a scenario from the bytes of its input YAML.
///
/// A hash rather than the file path, so the same scenario definition yields the same credentials wherever the
/// repository is checked out, and any edit to the scenario rolls them.
#[must_use]
pub fn scenario_seed(input_yaml: &[u8]) -> u64 {
    // Folded by hand rather than through `u64::from_be_bytes`, which this workspace's restriction lints reject
    // along with its little- and native-endian siblings. Only stability of the mapping matters here, not which
    // byte order it happens to be; `wrapping_shl` keeps the fold free of the overflow the lints also guard.
    Sha256::digest(input_yaml)
        .iter()
        .take(8)
        .fold(0_u64, |seed, &byte| seed.wrapping_shl(8) | u64::from(byte))
}

/// Build the scenario's `OpenObserve` root password from `seed`.
///
/// Reproducible by construction: seeding `ChaCha` fixes the whole stream, so a given seed always yields the
/// same password and the generated deployment files stay stable across machines and regenerations.
///
/// The shape is dictated by `OpenObserve`, which rejects a password that is not 8-128 characters with at least
/// one lowercase letter, one uppercase letter, one digit and one special character; a password sampled freely
/// from a charset satisfies that only by luck, so each class is placed deliberately and only the material
/// filling them is drawn from the RNG.
#[must_use]
pub fn scenario_password(seed: u64) -> String {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let first: String = Word().fake_with_rng(&mut rng);
    let second: String = Word().fake_with_rng(&mut rng);
    let digits: u32 = (1000..=9999_u32).fake_with_rng(&mut rng);
    let special_index: usize = (0..SPECIAL_CHARS.len()).fake_with_rng(&mut rng);
    let special = SPECIAL_CHARS.get(special_index).copied().unwrap_or('_');
    let mut capitalised = String::with_capacity(first.len());
    for (index, letter) in first.chars().enumerate() {
        if index == 0 {
            capitalised.extend(letter.to_uppercase());
        } else {
            capitalised.push(letter);
        }
    }

    format!("{capitalised}{special}{second}{digits}")
}
