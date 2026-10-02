//! RaptorQ (RFC 6330) coding of one segment as a single source block.

use raptorq::{
    EncodingPacket, ObjectTransmissionInformation, PayloadId, SourceBlockDecoder,
    SourceBlockEncoder,
};

/// Maximum source symbols in one RaptorQ source block.
pub const MAX_SOURCE_SYMBOLS: u32 = 56403;
const ESI_MASK: u64 = 0xFF_FFFF;

pub fn source_symbol_count(seg_len: u32, symbol_size: usize) -> u32 {
    (seg_len as u64)
        .div_ceil(symbol_size as u64)
        .min(u32::MAX as u64) as u32
}

/// Largest segment that fits in one source block with this symbol size.
pub fn max_segment_len(symbol_size: usize) -> u64 {
    MAX_SOURCE_SYMBOLS as u64 * symbol_size as u64
}

fn oti(seg_len: u32, symbol_size: usize) -> ObjectTransmissionInformation {
    ObjectTransmissionInformation::new(seg_len as u64, symbol_size as u16, 1, 1, 1)
}

/// Produces encoding symbols for one segment.
pub struct SegmentEncoder {
    padded: Vec<u8>,
    symbol_size: usize,
    k: u32,
    inner: SourceBlockEncoder,
}

impl SegmentEncoder {
    /// `data` must be non-empty and at most [`max_segment_len`] bytes.
    pub fn new(data: &[u8], symbol_size: usize) -> Self {
        assert!(!data.is_empty(), "segment must not be empty");
        let k = source_symbol_count(data.len() as u32, symbol_size);
        assert!(k <= MAX_SOURCE_SYMBOLS, "segment too large for symbol size");
        let mut padded = data.to_vec();
        padded.resize(k as usize * symbol_size, 0);
        let inner = SourceBlockEncoder::new(0, &oti(data.len() as u32, symbol_size), &padded);
        SegmentEncoder {
            padded,
            symbol_size,
            k,
            inner,
        }
    }

    /// Number of source symbols `K`.
    pub fn k(&self) -> u32 {
        self.k
    }

    /// The `j`-th symbol of the endless stream: source symbols first, then repair.
    /// Returns `(esi, symbol)`.
    pub fn symbol(&self, j: u64) -> (u32, Vec<u8>) {
        let esi = (j & ESI_MASK) as u32;
        if esi < self.k {
            let start = esi as usize * self.symbol_size;
            (esi, self.padded[start..start + self.symbol_size].to_vec())
        } else {
            let packet = self.inner.repair_packets(esi - self.k, 1).remove(0);
            let (id, data) = packet.split();
            debug_assert_eq!(id.encoding_symbol_id(), esi);
            (esi, data)
        }
    }
}

/// Collects symbols of one segment until it can be reconstructed.
pub struct SegmentDecoder {
    inner: SourceBlockDecoder,
    seg_len: u32,
    symbol_size: usize,
    received: u32,
}

impl SegmentDecoder {
    pub fn new(seg_len: u32, symbol_size: usize) -> Self {
        let inner = SourceBlockDecoder::new(0, &oti(seg_len, symbol_size), seg_len as u64);
        SegmentDecoder {
            inner,
            seg_len,
            symbol_size,
            received: 0,
        }
    }

    pub fn k(&self) -> u32 {
        source_symbol_count(self.seg_len, self.symbol_size)
    }

    pub fn symbol_size(&self) -> usize {
        self.symbol_size
    }

    /// Symbols pushed so far (duplicates included).
    pub fn received(&self) -> u32 {
        self.received
    }

    /// Adds a symbol; returns the segment once it is fully decoded.
    pub fn push(&mut self, esi: u32, symbol: &[u8]) -> Option<Vec<u8>> {
        assert_eq!(symbol.len(), self.symbol_size, "symbol size mismatch");
        self.received += 1;
        let packet = EncodingPacket::new(PayloadId::new(0, esi), symbol.to_vec());
        self.inner.decode(std::iter::once(packet)).map(|mut data| {
            data.truncate(self.seg_len as usize);
            data
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 131 + 7) as u8).collect()
    }

    #[test]
    fn source_only() {
        let d = data(1000);
        let enc = SegmentEncoder::new(&d, 64);
        let mut dec = SegmentDecoder::new(1000, 64);
        let mut out = None;
        for j in 0..enc.k() as u64 {
            let (esi, s) = enc.symbol(j);
            out = dec.push(esi, &s);
        }
        assert_eq!(out.unwrap(), d);
    }

    #[test]
    fn repair_only_with_overhead() {
        let d = data(5000);
        let enc = SegmentEncoder::new(&d, 100);
        let k = enc.k() as u64;
        let mut dec = SegmentDecoder::new(5000, 100);
        let mut out = None;
        // Skip every source symbol; recover from repair symbols alone.
        for j in k..k * 2 + 10 {
            let (esi, s) = enc.symbol(j);
            if let Some(o) = dec.push(esi, &s) {
                out = Some(o);
                break;
            }
        }
        assert_eq!(out.unwrap(), d);
    }

    #[test]
    fn lossy_mix() {
        let d = data(20_000);
        let enc = SegmentEncoder::new(&d, 128);
        let mut dec = SegmentDecoder::new(20_000, 128);
        let mut out = None;
        for j in 0..1000u64 {
            if j % 3 == 1 {
                continue; // drop a third of everything
            }
            let (esi, s) = enc.symbol(j);
            if let Some(o) = dec.push(esi, &s) {
                out = Some(o);
                break;
            }
        }
        assert_eq!(out.unwrap(), d);
    }
}
