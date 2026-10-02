//! zstd behind one interface: the C library natively, ruzstd for WebAssembly.
//! Both produce standard zstd frames, so either side can read the other.

use std::io::{self, BufRead, Read, Write};

#[cfg(not(any(feature = "zstd-native", feature = "ruzstd")))]
compile_error!("enable the `zstd-native` or `ruzstd` feature of qrsend-core");

#[derive(Debug, thiserror::Error)]
pub enum BoundedError {
    #[error("decompressed data exceeds limit")]
    TooLarge,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Decompresses at most `limit` bytes; errors if the stream holds more.
pub fn decompress_bounded(data: &[u8], limit: usize) -> Result<Vec<u8>, BoundedError> {
    let mut out = Vec::new();
    decoder(data)?
        .take(limit as u64 + 1)
        .read_to_end(&mut out)?;
    if out.len() > limit {
        return Err(BoundedError::TooLarge);
    }
    Ok(out)
}

pub fn compress(data: &[u8], level: i32) -> io::Result<Vec<u8>> {
    let mut enc = Encoder::new(Vec::new(), level, 0)?;
    enc.write_all(data)?;
    enc.finish()
}

#[cfg(feature = "zstd-native")]
mod imp {
    use super::*;

    /// Streaming zstd encoder.
    pub struct Encoder<W: Write>(zstd::stream::write::Encoder<'static, W>);

    impl<W: Write> Encoder<W> {
        pub fn new(inner: W, level: i32, workers: u32) -> io::Result<Self> {
            let mut enc = zstd::stream::write::Encoder::new(inner, level)?;
            #[cfg(feature = "zstdmt")]
            if workers > 1 {
                enc.multithread(workers)?;
            }
            #[cfg(not(feature = "zstdmt"))]
            let _ = workers;
            enc.include_checksum(true)?;
            Ok(Encoder(enc))
        }

        pub fn finish(self) -> io::Result<W> {
            self.0.finish()
        }
    }

    impl<W: Write> Write for Encoder<W> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    pub fn decoder<'a, R: BufRead + 'a>(inner: R) -> io::Result<Box<dyn Read + 'a>> {
        let mut dec = zstd::stream::read::Decoder::with_buffer(inner)?;
        // Allow windows up to 2 GiB (long-distance matching on huge inputs).
        dec.window_log_max(31)?;
        Ok(Box::new(dec))
    }
}

#[cfg(all(feature = "ruzstd", not(feature = "zstd-native")))]
mod imp {
    use super::*;

    /// Buffers everything and compresses on `finish` (ruzstd's encoder pulls
    /// from a reader). Fine for the in-memory WebAssembly use.
    pub struct Encoder<W: Write> {
        inner: W,
        buf: Vec<u8>,
    }

    impl<W: Write> Encoder<W> {
        pub fn new(inner: W, _level: i32, _workers: u32) -> io::Result<Self> {
            Ok(Encoder {
                inner,
                buf: Vec::new(),
            })
        }

        pub fn finish(mut self) -> io::Result<W> {
            ruzstd::encoding::compress(
                &self.buf[..],
                &mut self.inner,
                ruzstd::encoding::CompressionLevel::Fastest,
            );
            self.inner.flush()?;
            Ok(self.inner)
        }
    }

    impl<W: Write> Write for Encoder<W> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.buf.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    pub fn decoder<'a, R: BufRead + 'a>(inner: R) -> io::Result<Box<dyn Read + 'a>> {
        let dec = ruzstd::decoding::StreamingDecoder::new(inner)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        Ok(Box::new(dec))
    }
}

pub use imp::{Encoder, decoder};
