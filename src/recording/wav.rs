//! 16-bit mono PCM WAV files that stay playable even if the program dies mid-write.
//!
//! The header is rewritten with the real length at every [`WavWriter::checkpoint`],
//! so an abrupt exit loses at most the audio since the last checkpoint. Anything
//! after that is still on disk and [`repair`] reclaims it on the next start.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const HEADER_LEN: u64 = 44;
const BYTES_PER_SAMPLE: u32 = 2;

pub struct WavWriter {
    out: BufWriter<File>,
    path: PathBuf,
    sample_rate: u32,
    data_bytes: u32,
}

impl WavWriter {
    /// Creates a new file. Fails rather than overwrite an existing one.
    pub fn create(path: PathBuf, sample_rate: u32) -> io::Result<Self> {
        let file = OpenOptions::new().write(true).create_new(true).open(&path)?;
        let mut out = BufWriter::with_capacity(64 * 1024, file);
        out.write_all(&header(sample_rate, 0))?;
        Ok(Self { out, path, sample_rate, data_bytes: 0 })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn data_bytes(&self) -> u32 {
        self.data_bytes
    }

    pub fn write(&mut self, samples: &[f32]) -> io::Result<()> {
        for s in samples {
            let value = (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
            self.out.write_all(&value.to_le_bytes())?;
            self.data_bytes += BYTES_PER_SAMPLE;
        }
        Ok(())
    }

    /// Makes everything written so far a valid, playable file. With `sync`, also waits
    /// until it is physically on disk (survives a power cut, not just a crash).
    pub fn checkpoint(&mut self, sync: bool) -> io::Result<()> {
        self.out.flush()?;
        let file = self.out.get_mut();
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header(self.sample_rate, self.data_bytes))?;
        file.seek(SeekFrom::End(0))?;
        if sync {
            file.sync_data()?;
        }
        Ok(())
    }

    /// Writes the final header and closes the file.
    pub fn finish(mut self) -> io::Result<()> {
        self.checkpoint(true)
    }
}

fn header(sample_rate: u32, data_bytes: u32) -> [u8; HEADER_LEN as usize] {
    let mut h = [0u8; HEADER_LEN as usize];
    let mut put = |at: usize, bytes: &[u8]| h[at..at + bytes.len()].copy_from_slice(bytes);
    put(0, b"RIFF");
    put(4, &(36 + data_bytes).to_le_bytes());
    put(8, b"WAVE");
    put(12, b"fmt ");
    put(16, &16u32.to_le_bytes()); // fmt chunk size
    put(20, &1u16.to_le_bytes()); // PCM
    put(22, &1u16.to_le_bytes()); // mono
    put(24, &sample_rate.to_le_bytes());
    put(28, &(sample_rate * BYTES_PER_SAMPLE).to_le_bytes()); // byte rate
    put(32, &(BYTES_PER_SAMPLE as u16).to_le_bytes()); // block align
    put(34, &16u16.to_le_bytes()); // bits per sample
    put(36, b"data");
    put(40, &data_bytes.to_le_bytes());
    h
}

/// Fixes a file written by [`WavWriter`] whose header is behind its real content
/// (the program stopped between checkpoints). Returns whether anything changed.
/// Files that were not written by this program are left alone.
pub fn repair(path: &Path) -> io::Result<bool> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut h = [0u8; HEADER_LEN as usize];
    if file.read_exact(&mut h).is_err() || h != header(read_u32(&h, 24), read_u32(&h, 40)) {
        return Ok(false);
    }

    let len = file.metadata()?.len();
    let payload = len - HEADER_LEN;
    // Whole samples only (a crash can cut one in half), within the format's 4 GiB limit.
    let real = (payload - payload % BYTES_PER_SAMPLE as u64).min((u32::MAX - 36) as u64) as u32;
    if read_u32(&h, 40) == real && len == HEADER_LEN + real as u64 {
        return Ok(false);
    }

    file.set_len(HEADER_LEN + real as u64)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&header(read_u32(&h, 24), real))?;
    file.sync_all()?;
    Ok(true)
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stated_len(path: &Path) -> u32 {
        let bytes = std::fs::read(path).unwrap();
        read_u32(&bytes, 40)
    }

    #[test]
    fn checkpoint_makes_the_header_match_the_content() {
        let dir = tempfile::tempdir().unwrap();
        let mut wav = WavWriter::create(dir.path().join("a.wav"), 48_000).unwrap();
        wav.write(&[0.5; 100]).unwrap();
        wav.checkpoint(false).unwrap();
        assert_eq!(stated_len(wav.path()), 200);
        let path = wav.path().to_owned();
        wav.finish().unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), HEADER_LEN + 200);
    }

    #[test]
    fn refuses_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        WavWriter::create(path.clone(), 48_000).unwrap().finish().unwrap();
        assert!(WavWriter::create(path, 48_000).is_err());
    }

    #[test]
    fn repair_recovers_audio_written_after_the_last_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let mut wav = WavWriter::create(dir.path().join("a.wav"), 48_000).unwrap();
        wav.write(&[0.5; 100]).unwrap();
        wav.checkpoint(false).unwrap();
        wav.write(&[0.5; 50]).unwrap();
        let path = wav.path().to_owned();
        // Simulate a crash: buffered data reaches the OS, but no final header is written.
        wav.out.flush().unwrap();
        std::mem::forget(wav);
        // ...and half a sample made it to disk.
        OpenOptions::new().append(true).open(&path).unwrap().write_all(&[1]).unwrap();

        assert_eq!(stated_len(&path), 200);
        assert!(repair(&path).unwrap());
        assert_eq!(stated_len(&path), 300);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), HEADER_LEN + 300);
        assert!(!repair(&path).unwrap(), "a healthy file is left alone");
    }

    #[test]
    fn repair_ignores_foreign_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("other.wav");
        std::fs::write(&path, b"not a wav file at all, but long enough to have a header").unwrap();
        assert!(!repair(&path).unwrap());
    }
}
