// SPDX-License-Identifier: MIT OR Apache-2.0
//! A minimal ZIP reader and writer for the application data archives: STORE and DEFLATE only,
//! no zip64 (entries and the archive stay below 4 GiB), no encryption, no data descriptors.
use std::io::{self, Read, Write};

const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const END_SIG: u32 = 0x0605_4b50;
const UTF8_FLAG: u16 = 0x0800;
const METHOD_STORE: u16 = 0;
const METHOD_DEFLATE: u16 = 8;
/// Names must be UTF-8 by feature rule; reject anything else at parse time.
const MAX_COMMENT: usize = 65_536;

fn unsupported(what: &str) -> String {
    format!("unsupported archive feature: {what}")
}

/// DOS date and time (local, by ZIP convention) for a Unix timestamp in seconds.
pub(crate) fn dos_datetime(secs: u64) -> (u16, u16) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days: valid for the whole unsigned timestamp range here.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let year = y.clamp(1980, 2107) as u16;
    let date = ((year - 1980) << 9) | ((m as u16) << 5) | d as u16;
    let time = ((rem / 3600) as u16) << 11 | ((rem % 3600 / 60) as u16) << 5 | (rem % 60) as u16;
    (date, time)
}

struct CentralEntry {
    name: String,
    method: u16,
    crc32: u32,
    compressed_size: u32,
    uncompressed_size: u32,
    offset: u32,
}

/// A parsed central directory over an archive held fully in memory by the caller.
pub struct ZipArchive {
    entries: Vec<CentralEntry>,
}

impl ZipArchive {
    /// Parses the central directory. Rejects multi-disk archives, encryption, zip64 and
    /// non-UTF-8 names; directory entries keep their trailing `/` in `name`.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let end = find_eocd(bytes)?;
        let disk = u16le(bytes, end + 4);
        if disk != 0 {
            return Err(unsupported("multi-disk archive"));
        }
        let count = u16le(bytes, end + 10) as usize;
        let offset = u32le(bytes, end + 16) as usize;
        let mut cursor = offset;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            if bytes.len() < cursor + 46 || u32le(bytes, cursor) != CENTRAL_SIG {
                return Err("corrupt archive: bad central directory entry".into());
            }
            let flags = u16le(bytes, cursor + 8);
            if flags & 1 != 0 {
                return Err(unsupported("encrypted entry"));
            }
            let method = u16le(bytes, cursor + 10);
            if method != METHOD_STORE && method != METHOD_DEFLATE {
                return Err(unsupported("compression method"));
            }
            let sizes = (
                u32le(bytes, cursor + 20),
                u32le(bytes, cursor + 24),
                u32le(bytes, cursor + 28),
            );
            if sizes.0 == u32::MAX || sizes.1 == u32::MAX || sizes.2 == u32::MAX {
                return Err(unsupported("zip64 (entries above 4 GiB)"));
            }
            let name_len = u16le(bytes, cursor + 28) as usize;
            let extra_len = u16le(bytes, cursor + 30) as usize;
            let comment_len = u16le(bytes, cursor + 32) as usize;
            let name_bytes = bytes
                .get(cursor + 46..cursor + 46 + name_len)
                .ok_or_else(|| "corrupt archive: truncated entry name".to_string())?;
            let name = String::from_utf8(name_bytes.to_vec())
                .map_err(|_| "unsupported archive feature: non-UTF-8 entry name".to_string())?;
            entries.push(CentralEntry {
                name,
                method,
                crc32: u32le(bytes, cursor + 16),
                compressed_size: sizes.0,
                uncompressed_size: sizes.1,
                offset: sizes.2,
            });
            cursor += 46 + name_len + extra_len + comment_len;
        }
        Ok(Self { entries })
    }

    /// The central directory entries, in archive order.
    pub fn entries(&self) -> impl Iterator<Item = ZipEntry<'_>> {
        self.entries.iter().map(|entry| ZipEntry {
            name: entry.name.as_str(),
            method: entry.method,
            crc32: entry.crc32,
            compressed_size: entry.compressed_size,
            uncompressed_size: entry.uncompressed_size,
        })
    }

    /// Reads and decompresses one entry, verifying sizes and CRC32; `cap` bounds the
    /// uncompressed size (zip-bomb protection: the caller picks it before decompressing).
    pub fn read(&self, bytes: &[u8], name: &str, cap: u64) -> Result<Vec<u8>, String> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| format!("archive has no entry {name}"))?;
        let start = entry.offset as usize;
        if bytes.len() < start + 30 || u32le(bytes, start) != LOCAL_SIG {
            return Err("corrupt archive: bad local header".into());
        }
        let name_len = u16le(bytes, start + 26) as usize;
        let extra_len = u16le(bytes, start + 28) as usize;
        let data = bytes
            .get(start + 30 + name_len + extra_len..)
            .and_then(|rest| rest.get(..entry.compressed_size as usize))
            .ok_or_else(|| "corrupt archive: truncated entry data".to_string())?;
        let mut out =
            Vec::with_capacity(u64::from(entry.uncompressed_size).min(cap).min(64 << 20) as usize);
        match entry.method {
            METHOD_STORE => out.extend_from_slice(data),
            METHOD_DEFLATE => {
                let mut decoder = flate2::read::DeflateDecoder::new(data).take(cap + 1);
                decoder
                    .read_to_end(&mut out)
                    .map_err(|error| format!("corrupt archive: {name}: {error}"))?;
            }
            _ => return Err(unsupported("compression method")),
        }
        if out.len() as u64 > cap {
            return Err(format!("entry {name} exceeds the size limit"));
        }
        if out.len() as u64 != entry.uncompressed_size as u64 {
            return Err(format!("corrupt archive: {name}: size mismatch"));
        }
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&out);
        if hasher.finalize() != entry.crc32 {
            return Err(format!("corrupt archive: {name}: checksum mismatch"));
        }
        Ok(out)
    }
}

/// A read-only view of one central directory entry.
pub struct ZipEntry<'a> {
    pub name: &'a str,
    pub method: u16,
    pub crc32: u32,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
}

impl ZipEntry<'_> {
    /// An archive record of a directory rather than of a file.
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }
}

/// Searches backwards for the end-of-central-directory record whose comment spans to the end.
fn find_eocd(bytes: &[u8]) -> Result<usize, String> {
    if bytes.len() < 22 {
        return Err("corrupt archive: too short".into());
    }
    let low = bytes.len().saturating_sub(22 + MAX_COMMENT);
    let mut end = bytes.len() - 22;
    loop {
        if u32le(bytes, end) == END_SIG && u16le(bytes, end + 20) as usize == bytes.len() - end - 22
        {
            return Ok(end);
        }
        if end == low {
            return Err("corrupt archive: no end record".into());
        }
        end -= 1;
    }
}

fn u16le(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32le(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Builds an archive in memory: local headers with known sizes (no data descriptors), a central
/// directory and an end record. Empty files are stored, everything else deflated.
pub struct ZipWriter<W: io::Write> {
    inner: W,
    offset: u64,
    central: Vec<u8>,
    count: u16,
    dos_date: u16,
    dos_time: u16,
}

impl<W: io::Write> ZipWriter<W> {
    /// Every entry is stamped with the DOS date and time of `unix_secs`.
    pub fn new(inner: W, unix_secs: u64) -> Self {
        let (dos_date, dos_time) = dos_datetime(unix_secs);
        Self {
            inner,
            offset: 0,
            central: Vec::new(),
            count: 0,
            dos_date,
            dos_time,
        }
    }

    /// Adds one file. The name must be UTF-8 and free of `\`, NUL and drive letters; the
    /// archive-level path rules live with the caller. A trailing `/` records a directory.
    pub fn add(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let name = name.as_bytes();
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(bytes);
        let crc = hasher.finalize();
        let (method, payload): (u16, Vec<u8>) = if bytes.is_empty() {
            (METHOD_STORE, Vec::new())
        } else {
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(bytes)?;
            (METHOD_DEFLATE, encoder.finish()?)
        };
        let local_offset = self.offset;
        let header = local_header(
            name,
            method,
            crc,
            payload.len(),
            bytes.len(),
            self.dos_date,
            self.dos_time,
        );
        self.inner.write_all(&header)?;
        self.inner.write_all(&payload)?;
        self.offset += header.len() as u64 + payload.len() as u64;
        self.central.extend_from_slice(&central_header(
            name,
            method,
            crc,
            payload.len(),
            bytes.len(),
            local_offset,
            self.dos_date,
            self.dos_time,
        ));
        // An archive of more than 65_535 files needs zip64, which this writer does not do.
        self.count = self.count.saturating_add(1);
        Ok(())
    }

    /// Writes the central directory and end record and returns the inner writer.
    pub fn finish(mut self) -> io::Result<W> {
        let central_offset = self.offset;
        let central_size = self.central.len() as u64;
        self.inner.write_all(&self.central)?;
        let mut end = Vec::with_capacity(22);
        end.extend_from_slice(&END_SIG.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        end.extend_from_slice(&self.count.to_le_bytes());
        end.extend_from_slice(&self.count.to_le_bytes());
        end.extend_from_slice(&(central_size as u32).to_le_bytes());
        end.extend_from_slice(&(central_offset as u32).to_le_bytes());
        end.extend_from_slice(&0u16.to_le_bytes());
        self.inner.write_all(&end)?;
        self.inner.flush()?;
        Ok(self.inner)
    }
}

fn local_header(
    name: &[u8],
    method: u16,
    crc: u32,
    compressed: usize,
    uncompressed: usize,
    date: u16,
    time: u16,
) -> Vec<u8> {
    let mut h = Vec::with_capacity(30 + name.len());
    h.extend_from_slice(&LOCAL_SIG.to_le_bytes());
    h.extend_from_slice(&20u16.to_le_bytes()); // version needed
    h.extend_from_slice(&UTF8_FLAG.to_le_bytes());
    h.extend_from_slice(&method.to_le_bytes());
    h.extend_from_slice(&time.to_le_bytes());
    h.extend_from_slice(&date.to_le_bytes());
    h.extend_from_slice(&crc.to_le_bytes());
    h.extend_from_slice(&(compressed as u32).to_le_bytes());
    h.extend_from_slice(&(uncompressed as u32).to_le_bytes());
    h.extend_from_slice(&(name.len() as u16).to_le_bytes());
    h.extend_from_slice(&0u16.to_le_bytes()); // extra length
    h.extend_from_slice(name);
    h
}

#[allow(clippy::too_many_arguments)]
fn central_header(
    name: &[u8],
    method: u16,
    crc: u32,
    compressed: usize,
    uncompressed: usize,
    offset: u64,
    date: u16,
    time: u16,
) -> Vec<u8> {
    let mut h = Vec::with_capacity(46 + name.len());
    h.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
    h.extend_from_slice(&((3u16 << 8) | 20).to_le_bytes()); // made by unix, version 2.0
    h.extend_from_slice(&20u16.to_le_bytes());
    h.extend_from_slice(&UTF8_FLAG.to_le_bytes());
    h.extend_from_slice(&method.to_le_bytes());
    h.extend_from_slice(&time.to_le_bytes());
    h.extend_from_slice(&date.to_le_bytes());
    h.extend_from_slice(&crc.to_le_bytes());
    h.extend_from_slice(&(compressed as u32).to_le_bytes());
    h.extend_from_slice(&(uncompressed as u32).to_le_bytes());
    h.extend_from_slice(&(name.len() as u16).to_le_bytes());
    h.extend_from_slice(&0u16.to_le_bytes()); // extra
    h.extend_from_slice(&0u16.to_le_bytes()); // comment
    h.extend_from_slice(&0u16.to_le_bytes()); // disk number
    h.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
    h.extend_from_slice(&0o100_644u32.to_le_bytes()); // external: regular file
    h.extend_from_slice(&(offset as u32).to_le_bytes());
    h.extend_from_slice(name);
    h
}
