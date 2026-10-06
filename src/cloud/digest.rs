//! Hashes and randomness. SHA-256, HMAC and the CSPRNG come from `ring`,
//! which the TLS stack already builds; MD5 is written out here because
//! Daytona names build contexts by an MD5 of their contents.

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256(data: &[u8]) -> Vec<u8> {
    ring::digest::digest(&ring::digest::SHA256, data).as_ref().to_vec()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&sha256(data))
}

/// An incremental SHA-256 for inputs made of several parts.
pub struct Sha256(ring::digest::Context);

impl Default for Sha256 {
    fn default() -> Self {
        Self(ring::digest::Context::new(&ring::digest::SHA256))
    }
}

impl Sha256 {
    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
    pub fn hex(self) -> String {
        hex(self.0.finish().as_ref())
    }
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    ring::hmac::sign(&ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key), data).as_ref().to_vec()
}

/// `n` random bytes from the operating system, hex encoded.
pub fn random_hex(n: usize) -> String {
    use ring::rand::SecureRandom;
    let mut buf = vec![0u8; n];
    ring::rand::SystemRandom::new().fill(&mut buf).expect("the OS random number generator failed");
    hex(&buf)
}

/// Compares secrets without leaking where they first differ.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// MD5 (RFC 1321). Only an identifier for build contexts, never security.
#[derive(Clone)]
pub struct Md5 {
    state: [u32; 4],
    buffer: Vec<u8>,
    length: u64,
}

impl Default for Md5 {
    fn default() -> Self {
        Self { state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476], buffer: Vec::with_capacity(64), length: 0 }
    }
}

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15,
    21,
];

impl Md5 {
    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        if !self.buffer.is_empty() {
            let take = (64 - self.buffer.len()).min(data.len());
            self.buffer.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buffer.len() == 64 {
                let block: [u8; 64] = self.buffer[..].try_into().unwrap();
                self.block(&block);
                self.buffer.clear();
            }
        }
        while data.len() >= 64 {
            self.block(data[..64].try_into().unwrap());
            data = &data[64..];
        }
        self.buffer.extend_from_slice(data);
    }

    pub fn hex(mut self) -> String {
        let bits = self.length.wrapping_mul(8);
        let mut pad = vec![0x80u8];
        while (self.buffer.len() + pad.len()) % 64 != 56 {
            pad.push(0);
        }
        pad.extend_from_slice(&bits.to_le_bytes());
        let length = self.length;
        self.update(&pad);
        self.length = length;
        hex(&self.state.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>())
    }

    fn block(&mut self, block: &[u8; 64]) {
        let m: Vec<u32> = block.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        let [mut a, mut b, mut c, mut d] = self.state;
        for (i, shift) in S.iter().enumerate() {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let k = ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32;
            let rotated = a.wrapping_add(f).wrapping_add(k).wrapping_add(m[g]).rotate_left(*shift);
            (a, d, c, b) = (d, c, b, b.wrapping_add(rotated));
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md5(data: &[u8]) -> String {
        let mut h = Md5::default();
        h.update(data);
        h.hex()
    }

    #[test]
    fn md5_matches_rfc_1321_vectors() {
        assert_eq!(md5(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(md5(b"message digest"), "f96b697d7cb7938d525a2f31aaf161d0");
        assert_eq!(
            md5(b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
        // Split updates give the same answer as one.
        let mut h = Md5::default();
        for chunk in b"12345678901234567890123456789012345678901234567890123456789012345678901234567890".chunks(7) {
            h.update(chunk);
        }
        assert_eq!(h.hex(), "57edf4a22be3c955ac49da2e2107b67a");
    }

    #[test]
    fn sha256_and_hmac_vectors() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        // RFC 4231 test case 2.
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        let mut s = Sha256::default();
        s.update(b"a");
        s.update(b"bc");
        assert_eq!(s.hex(), sha256_hex(b"abc"));
    }

    #[test]
    fn random_and_comparison() {
        let a = random_hex(32);
        assert_eq!(a.len(), 64);
        assert_ne!(a, random_hex(32));
        assert!(constant_time_eq(b"same", b"same"));
        assert!(!constant_time_eq(b"same", b"sane"));
        assert!(!constant_time_eq(b"short", b"longer"));
    }
}
