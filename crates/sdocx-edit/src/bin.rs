//! Little-endian byte helpers, Samsung-style identifiers and hashes.

use sha2::{Digest, Sha256};

#[derive(Default, Debug, Clone)]
pub struct Buf(pub Vec<u8>);

impl Buf {
    pub fn new() -> Self {
        Self(Vec::new())
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn f64(&mut self, v: f64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }
    pub fn zeros(&mut self, n: usize) -> &mut Self {
        self.0.resize(self.0.len() + n, 0);
        self
    }
    /// `u16` UTF-16 code-unit count followed by UTF-16LE text.
    pub fn str16(&mut self, s: &str) -> &mut Self {
        let units: Vec<u16> = s.encode_utf16().take(0xFFFF).collect();
        self.u16(units.len() as u16);
        for u in units {
            self.u16(u);
        }
        self
    }
    /// `u32` UTF-16 code-unit count followed by UTF-16LE text.
    pub fn str32(&mut self, s: &str) -> &mut Self {
        let units: Vec<u16> = s.encode_utf16().collect();
        self.u32(units.len() as u32);
        for u in units {
            self.u16(u);
        }
        self
    }
    pub fn put_u32_at(&mut self, at: usize, v: u32) {
        self.0[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// Samsung identity digest: SHA-256 of `uuid` followed by the decimal modification time.
pub fn identity_hash(id: &str, time: u64) -> [u8; 32] {
    sha256(format!("{id}{}", time as i64).as_bytes())
}

/// Current time in microseconds, the unit Samsung Notes uses for object times.
pub fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

/// A version-1 (time based) UUID, the style Samsung Notes uses for pages, layers and objects.
pub fn samsung_uuid() -> String {
    use rand::RngCore;
    let mut node = [0u8; 6];
    rand::rng().fill_bytes(&mut node);
    node[0] |= 0x01; // multicast bit: random node id
    let ctx = uuid::ContextV1::new_random();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let ts = uuid::Timestamp::from_unix(&ctx, now.as_secs(), now.subsec_nanos());
    uuid::Uuid::new_v1(ts, &node).to_string()
}

pub fn rd_u16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
pub fn rd_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
pub fn rd_u64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}
/// Reads a `u16`-count UTF-16LE string, returning it and the offset after it.
pub fn rd_str16(b: &[u8], at: usize) -> Option<(String, usize)> {
    let n = rd_u16(b, at)? as usize;
    let start = at + 2;
    let raw = b.get(start..start + n * 2)?;
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    Some((String::from_utf16_lossy(&units), start + n * 2))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uuid_is_v1() {
        let id = samsung_uuid();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "1");
    }
}
