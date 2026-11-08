#![allow(dead_code)]
use std::path::PathBuf;

pub const OPENING: &str = "XXXX/XXXX/XXXX/XXXX - R1C1D1W1P1T1L1E1r1c1d1w1p1t1l1e1 0 0";
pub const VALUES: [f64; 8] = [6., 2., 3., 4., 5., 7., 8., 10.];

pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    pub fn index(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

// Same deterministic nonconstant weight generator as the inference reference.
pub fn random_bytes(hidden: usize) -> Vec<u8> {
    let mut rng = Rng(7919);
    let mut bytes = b"JFNN".to_vec();
    for n in [2, 384, hidden, 16, 32] {
        bytes.extend((n as u32).to_le_bytes());
    }
    let mut append = |count: usize, scale: f32, offset: f32| {
        for _ in 0..count {
            let v = offset + scale * (rng.index(65536) as f32 / 32768. - 1.);
            bytes.extend(v.to_le_bytes());
        }
    };
    append(384 * hidden, 0.06, 0.);
    append(hidden, 0., 0.5);
    append(32 * hidden, 1. / (2. * hidden as f32).sqrt(), 0.);
    append(16, 0.02, 0.1);
    append(512, 0.25, 0.);
    append(32, 0.02, 0.1);
    append(32, 0.3, 0.);
    append(1, 0.1, 0.);
    bytes
}

pub fn constant_bytes(logit: f32) -> Vec<u8> {
    let mut bytes = random_bytes(1);
    bytes[24..].fill(0);
    let end = bytes.len();
    bytes[end - 4..].copy_from_slice(&logit.to_le_bytes());
    bytes
}

pub fn external_net() -> Option<PathBuf> {
    match std::env::var_os("MISTY_FLIP_JUNGLE_TEST_NET") {
        Some(path) => Some(path.into()),
        None => {
            eprintln!("SKIP trained-net check: MISTY_FLIP_JUNGLE_TEST_NET is unset");
            None
        }
    }
}

pub struct TestFile(pub PathBuf);
impl TestFile {
    pub fn new(bytes: &[u8]) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "jungle test {} {}.jfnn",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&path, bytes).unwrap();
        Self(path)
    }
}
impl Drop for TestFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).unwrap();
    }
}
