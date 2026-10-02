//! zstd helpers shared by the manifest and body codecs.

use std::io::{self, Read, Write};

pub fn compress(data: &[u8], level: i32) -> io::Result<Vec<u8>> {
    zstd::stream::encode_all(data, level)
}

#[derive(Debug, thiserror::Error)]
pub enum BoundedError {
    #[error("decompressed data exceeds limit")]
    TooLarge,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Decompresses at most `limit` bytes; errors if the stream holds more.
pub fn decompress_bounded(data: &[u8], limit: usize) -> Result<Vec<u8>, BoundedError> {
    let decoder = zstd::stream::read::Decoder::new(data)?;
    let mut out = Vec::new();
    decoder.take(limit as u64 + 1).read_to_end(&mut out)?;
    if out.len() > limit {
        return Err(BoundedError::TooLarge);
    }
    Ok(out)
}

/// Streaming zstd encoder used for bodies.
pub fn encoder<W: Write>(
    inner: W,
    level: i32,
    workers: u32,
) -> io::Result<zstd::stream::write::Encoder<'static, W>> {
    let mut enc = zstd::stream::write::Encoder::new(inner, level)?;
    #[cfg(feature = "zstdmt")]
    if workers > 1 {
        enc.multithread(workers)?;
    }
    #[cfg(not(feature = "zstdmt"))]
    let _ = workers;
    enc.include_checksum(true)?;
    Ok(enc)
}

/// Streaming zstd decoder used for bodies.
pub fn decoder<R: io::BufRead>(inner: R) -> io::Result<zstd::stream::read::Decoder<'static, R>> {
    let mut dec = zstd::stream::read::Decoder::with_buffer(inner)?;
    // Allow windows up to 2 GiB (long-distance matching on huge inputs).
    dec.window_log_max(31)?;
    Ok(dec)
}
