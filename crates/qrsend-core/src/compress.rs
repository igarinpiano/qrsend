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

    /// Input is compressed in independent frames of this size, so memory use
    /// stays bounded however large the payload is. (ruzstd's encoder pulls
    /// from a reader and works one frame at a time.)
    const CHUNK: usize = if cfg!(test) { 1000 } else { 4 << 20 };

    pub struct Encoder<W: Write> {
        inner: W,
        buf: Vec<u8>,
        wrote_frame: bool,
    }

    impl<W: Write> Encoder<W> {
        pub fn new(inner: W, _level: i32, _workers: u32) -> io::Result<Self> {
            Ok(Encoder {
                inner,
                buf: Vec::new(),
                wrote_frame: false,
            })
        }

        fn emit(&mut self) {
            ruzstd::encoding::compress(
                &self.buf[..],
                &mut self.inner,
                ruzstd::encoding::CompressionLevel::Fastest,
            );
            self.buf.clear();
            self.wrote_frame = true;
        }

        pub fn finish(mut self) -> io::Result<W> {
            // Always at least one frame, so empty input is still valid zstd.
            if !self.buf.is_empty() || !self.wrote_frame {
                self.emit();
            }
            self.inner.flush()?;
            Ok(self.inner)
        }
    }

    impl<W: Write> Write for Encoder<W> {
        fn write(&mut self, mut data: &[u8]) -> io::Result<usize> {
            let n = data.len();
            while !data.is_empty() {
                let take = (CHUNK - self.buf.len()).min(data.len());
                self.buf.extend_from_slice(&data[..take]);
                data = &data[take..];
                if self.buf.len() == CHUNK {
                    self.emit();
                }
            }
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    type Frame<R> = ruzstd::decoding::StreamingDecoder<R, ruzstd::decoding::FrameDecoder>;

    /// Decodes consecutive zstd frames (ruzstd's decoder handles one).
    struct Frames<R: BufRead>(Option<Frame<R>>);

    fn invalid(e: impl std::fmt::Display) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, e.to_string())
    }

    impl<R: BufRead> Read for Frames<R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            loop {
                let Some(frame) = self.0.as_mut() else {
                    return Ok(0);
                };
                let n = frame.read(buf)?;
                if n > 0 {
                    return Ok(n);
                }
                let mut inner = self.0.take().unwrap().into_inner();
                if inner.fill_buf()?.is_empty() {
                    return Ok(0);
                }
                self.0 = Some(ruzstd::decoding::StreamingDecoder::new(inner).map_err(invalid)?);
            }
        }
    }

    pub fn decoder<'a, R: BufRead + 'a>(inner: R) -> io::Result<Box<dyn Read + 'a>> {
        Ok(Box::new(Frames(Some(
            ruzstd::decoding::StreamingDecoder::new(inner).map_err(invalid)?,
        ))))
    }
}

pub use imp::{Encoder, decoder};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_across_chunk_boundaries() {
        for len in [0usize, 1, 999, 1000, 1001, 2500, 10_000] {
            let data: Vec<u8> = (0..len).map(|i| (i / 7) as u8).collect();
            let mut enc = Encoder::new(Vec::new(), 3, 0).unwrap();
            for part in data.chunks(333) {
                enc.write_all(part).unwrap();
            }
            let packed = enc.finish().unwrap();
            assert_eq!(decompress_bounded(&packed, len).unwrap(), data, "len {len}");
        }
    }

    #[test]
    fn concatenated_frames_decode() {
        let mut packed = compress(b"hello ", 3).unwrap();
        packed.extend(compress(b"world", 3).unwrap());
        assert_eq!(decompress_bounded(&packed, 100).unwrap(), b"hello world");
    }
}
