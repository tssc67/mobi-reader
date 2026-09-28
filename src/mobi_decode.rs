//! Independent, bounded parsing of the public Palm Database/MOBI6 file format.
//! Format reference: https://wiki.mobileread.com/wiki/MOBI
//! The MIT-licensed mobi-rs format notes were consulted for HUFF/CDIC field meanings.
//! This module does not contain Calibre or KindleUnpack code.
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::File,
    io::Read,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_INPUT: usize = 256 * 1024 * 1024;
const MAX_TEXT: usize = 64 * 1024 * 1024;
const MAX_IMAGE: usize = 8 * 1024 * 1024;
const MAX_IMAGES: usize = 64 * 1024 * 1024;
const MAX_PHRASES: usize = 262144;
const MAX_PHRASE: usize = 1024 * 1024;
const MAX_DICT_CACHE: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub struct MobiImage {
    /// One-based image record number used by legacy HTML's recindex attribute.
    pub recindex: usize,
    pub bytes: Vec<u8>,
    pub media_type: &'static str,
    pub extension: &'static str,
}
#[derive(Debug)]
pub struct DecodedMobi {
    /// Uncompressed original text bytes; filepos offsets refer to these bytes.
    pub text: Vec<u8>,
    pub encoding: u32,
    pub title: String,
    pub authors: Vec<String>,
    pub images: Vec<MobiImage>,
    /// One-based recindex, matching an image's recindex (not its vector offset).
    pub cover: Option<usize>,
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Acquire), "MOBI import cancelled");
    Ok(())
}
fn slice(bytes: &[u8], start: usize, length: usize) -> Result<&[u8]> {
    let end = start.checked_add(length).context("MOBI offset overflow")?;
    bytes
        .get(start..end)
        .context("MOBI file contains a truncated or invalid offset")
}
fn be16(bytes: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(slice(bytes, at, 2)?.try_into()?))
}
fn be32(bytes: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(slice(bytes, at, 4)?.try_into()?))
}
fn text_string(bytes: &[u8], encoding: u32) -> Result<String> {
    match encoding {
        65001 => Ok(std::str::from_utf8(bytes)
            .context("MOBI metadata is not valid UTF-8")?
            .trim_matches('\0')
            .trim()
            .to_owned()),
        1252 => Ok(encoding_rs::WINDOWS_1252
            .decode_without_bom_handling(bytes)
            .0
            .trim_matches('\0')
            .trim()
            .to_owned()),
        _ => bail!(
            "Unsupported MOBI character encoding {encoding}; supported encodings are UTF-8 and Windows-1252"
        ),
    }
}

pub fn decode(source: &Path, cancel: &AtomicBool) -> Result<DecodedMobi> {
    check_cancel(cancel)?;
    let mut file = File::open(source).context("Cannot open MOBI source")?;
    ensure!(
        file.metadata()?.is_file(),
        "Choose a MOBI file, not a directory"
    );
    ensure!(
        file.metadata()?.len() <= MAX_INPUT as u64,
        "MOBI source exceeds the 256 MiB input limit"
    );
    let mut bytes = Vec::new();
    let mut buffer = [0; 65536];
    loop {
        check_cancel(cancel)?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        ensure!(
            bytes.len() <= MAX_INPUT.saturating_sub(count),
            "MOBI source exceeds the 256 MiB input limit"
        );
        bytes.extend_from_slice(&buffer[..count]);
    }
    decode_bytes(&bytes, cancel)
}

fn decode_bytes(bytes: &[u8], cancel: &AtomicBool) -> Result<DecodedMobi> {
    check_cancel(cancel)?;
    ensure!(
        bytes.len() <= MAX_INPUT,
        "MOBI source exceeds the 256 MiB input limit"
    );
    let signature = slice(bytes, 60, 8)?;
    ensure!(
        signature == b"BOOKMOBI" || signature == b"TEXtREAd",
        "This file is not a MOBI/PalmDOC book"
    );
    ensure!(
        be32(bytes, 72)? == 0,
        "MOBI files with chained Palm Database record lists are unsupported"
    );
    let count = be16(bytes, 76)? as usize;
    ensure!(count >= 2, "MOBI has no text records");
    let table_end = 78usize
        .checked_add(count * 8)
        .context("MOBI record table overflow")?;
    slice(bytes, 78, count * 8)?;
    let mut offsets = Vec::with_capacity(count + 1);
    for index in 0..count {
        let offset = be32(bytes, 78 + index * 8)? as usize;
        ensure!(
            offset >= table_end && offset <= bytes.len(),
            "MOBI record points outside the file"
        );
        ensure!(
            offsets.last().is_none_or(|last| offset >= *last),
            "MOBI record offsets are not ordered"
        );
        offsets.push(offset);
    }
    offsets.push(bytes.len());
    let record = |index: usize| -> Result<&[u8]> {
        ensure!(index < count, "MOBI references a missing record");
        slice(bytes, offsets[index], offsets[index + 1] - offsets[index])
    };
    let header = record(0)?;
    let compression = be16(header, 0)?;
    ensure!(
        matches!(compression, 1 | 2 | 17480),
        "Unsupported MOBI compression {compression}"
    );
    let declared_length = be32(header, 4)? as usize;
    ensure!(
        declared_length > 0 && declared_length <= MAX_TEXT,
        "MOBI text is empty or exceeds the 64 MiB limit"
    );
    let text_records = be16(header, 8)? as usize;
    ensure!(
        text_records > 0 && text_records < count,
        "Invalid MOBI text record count"
    );
    let record_size = be16(header, 10)? as usize;
    ensure!(record_size > 0, "Invalid MOBI text record size");
    if signature == b"BOOKMOBI" {
        ensure!(
            be16(header, 12)? == 0,
            "Encrypted or DRM-protected MOBI books are unsupported"
        );
    }
    let mut encoding = 1252;
    let mut title = String::new();
    let mut authors = Vec::new();
    let mut first_image = None;
    let mut cover = None;
    let mut extra_flags = 0;
    let mut huff_range = None;
    if signature == b"BOOKMOBI" {
        ensure!(
            slice(header, 16, 4)? == b"MOBI",
            "MOBI record zero has no MOBI header"
        );
        let header_length = be32(header, 20)? as usize;
        ensure!(header_length >= 24, "MOBI header is too short");
        slice(header, 16, header_length)?;
        let version = be32(header, 36)?;
        ensure!(
            version < 8,
            "Pure KF8/MOBI8 books are not supported by the built-in converter; dual MOBI books use their legacy MOBI6 section"
        );
        ensure!(version > 0, "Invalid MOBI format version");
        encoding = be32(header, 28)?;
        ensure!(
            matches!(encoding, 65001 | 1252),
            "Unsupported MOBI character encoding {encoding}; supported encodings are UTF-8 and Windows-1252"
        );
        if header_length >= 76 {
            let at = be32(header, 84)? as usize;
            let length = be32(header, 88)? as usize;
            if length > 0 {
                title = text_string(slice(header, at, length)?, encoding)?;
            }
        }
        if header_length >= 96 {
            let image = be32(header, 108)? as usize;
            if image != u32::MAX as usize {
                ensure!(
                    image > text_records && image <= count,
                    "Invalid first MOBI image record"
                );
                first_image = Some(image);
            }
        }
        if header_length >= 104 && compression == 17480 {
            let at = be32(header, 112)? as usize;
            let length = be32(header, 116)? as usize;
            let end = at
                .checked_add(length)
                .context("HUFF record range overflow")?;
            ensure!(
                length >= 2 && at > text_records && end <= count,
                "Invalid MOBI HUFF/CDIC record range"
            );
            huff_range = Some(at..end);
        }
        if header_length >= 228 {
            extra_flags = be16(header, 242)?;
        }
        if header_length >= 116 && be32(header, 128)? & 0x40 != 0 {
            let exth = 16 + header_length;
            ensure!(
                slice(header, exth, 4)? == b"EXTH",
                "MOBI metadata claims a missing EXTH header"
            );
            let length = be32(header, exth + 4)? as usize;
            ensure!(length >= 12, "Invalid EXTH header length");
            let data = slice(header, exth, length)?;
            let entries = be32(data, 8)? as usize;
            ensure!(
                entries <= (length - 12) / 8,
                "Invalid EXTH metadata entry count"
            );
            let mut at = 12;
            for _ in 0..entries {
                check_cancel(cancel)?;
                let kind = be32(data, at)?;
                let size = be32(data, at + 4)? as usize;
                ensure!(size >= 8, "Invalid EXTH metadata entry size");
                let value = slice(data, at + 8, size - 8)?;
                match kind {
                    100 => {
                        let author = text_string(value, encoding)?;
                        if !author.is_empty() {
                            authors.push(author);
                        }
                    }
                    503 => {
                        let name = text_string(value, encoding)?;
                        if !name.is_empty() {
                            title = name;
                        }
                    }
                    201 => {
                        ensure!(value.len() == 4, "Invalid MOBI cover offset");
                        cover = Some(
                            (be32(value, 0)? as usize)
                                .checked_add(1)
                                .context("MOBI cover offset overflow")?,
                        );
                    }
                    122 => {
                        let fixed = text_string(value, encoding)?;
                        ensure!(
                            !matches!(fixed.to_ascii_lowercase().as_str(), "true" | "yes" | "1"),
                            "Fixed-layout MOBI books are unsupported"
                        );
                    }
                    _ => {}
                }
                at = at.checked_add(size).context("EXTH offset overflow")?;
            }
        }
    } else {
        ensure!(
            compression != 17480,
            "PalmDOC without a MOBI header cannot provide HUFF/CDIC dictionaries"
        );
    }
    let mut huff = if compression == 17480 {
        let range = huff_range.context("HUFF/CDIC compression has no dictionary records")?;
        let records = range.map(record).collect::<Result<Vec<_>>>()?;
        Some(Huff::new(&records, cancel)?)
    } else {
        None
    };
    let mut text = Vec::with_capacity(declared_length.min(1024 * 1024));
    for index in 1..=text_records {
        check_cancel(cancel)?;
        let raw = strip_trailing(record(index)?, extra_flags)?;
        let limit = record_size.min(MAX_TEXT - text.len());
        let decoded = match compression {
            1 => {
                ensure!(
                    raw.len() <= limit,
                    "MOBI text record exceeds its declared size"
                );
                raw.to_vec()
            }
            2 => palmdoc(raw, limit, cancel)?,
            _ => huff
                .as_mut()
                .context("Missing HUFF decoder")?
                .unpack(raw, limit, 0, cancel)?,
        };
        ensure!(
            decoded.len() <= declared_length.saturating_sub(text.len()),
            "MOBI decompressed text exceeds its declared length"
        );
        text.extend_from_slice(&decoded);
    }
    ensure!(
        text.len() == declared_length,
        "MOBI decompressed text length does not match its header"
    );
    if encoding == 65001 {
        std::str::from_utf8(&text).context("MOBI chapter text is not valid UTF-8")?;
    }
    let mut images = Vec::new();
    let mut total_images = 0;
    if let Some(first) = first_image {
        for index in first..count {
            check_cancel(cancel)?;
            let data = record(index)?;
            if let Some((media_type, extension)) = image_type(data) {
                ensure!(
                    data.len() <= MAX_IMAGE,
                    "MOBI image exceeds the 8 MiB limit"
                );
                total_images += data.len();
                ensure!(
                    total_images <= MAX_IMAGES,
                    "MOBI images exceed the 64 MiB limit"
                );
                images.push(MobiImage {
                    recindex: index - first + 1,
                    bytes: data.to_vec(),
                    media_type,
                    extension,
                });
            }
        }
    }
    cover = cover.filter(|cover| images.iter().any(|image| image.recindex == *cover));
    check_cancel(cancel)?;
    Ok(DecodedMobi {
        text,
        encoding,
        title,
        authors,
        images,
        cover,
    })
}

fn image_type(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", "png"))
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(("image/jpeg", "jpg"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(("image/gif", "gif"))
    } else if bytes.starts_with(b"BM") {
        Some(("image/bmp", "bmp"))
    } else if bytes.get(..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some(("image/webp", "webp"))
    } else {
        None
    }
}

fn strip_trailing(mut bytes: &[u8], flags: u16) -> Result<&[u8]> {
    for _ in 0..(flags >> 1).count_ones() {
        let mut size = 0usize;
        let mut consumed = 0;
        let mut terminated = false;
        for byte in bytes.iter().rev().take(4) {
            size |= ((byte & 0x7f) as usize) << (7 * consumed);
            consumed += 1;
            if byte & 0x80 != 0 {
                terminated = true;
                break;
            }
        }
        ensure!(
            terminated && size >= consumed && size <= bytes.len(),
            "Invalid MOBI trailing record data"
        );
        bytes = &bytes[..bytes.len() - size];
    }
    if flags & 1 != 0 {
        let last = bytes
            .last()
            .context("MOBI multibyte overlap trailer is missing")?;
        let size = (last & 3) as usize + 1;
        ensure!(
            size <= bytes.len(),
            "Invalid MOBI multibyte overlap trailer"
        );
        bytes = &bytes[..bytes.len() - size];
    }
    Ok(bytes)
}

fn palmdoc(bytes: &[u8], limit: usize, cancel: &AtomicBool) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if at.is_multiple_of(1024) {
            check_cancel(cancel)?;
        }
        let byte = bytes[at];
        at += 1;
        match byte {
            1..=8 => {
                let literal = slice(bytes, at, byte as usize)?;
                ensure!(
                    output.len() <= limit.saturating_sub(literal.len()) && literal.len() <= limit,
                    "PalmDOC expansion exceeds the text limit"
                );
                output.extend_from_slice(literal);
                at += literal.len();
            }
            0 | 9..=127 => {
                ensure!(
                    output.len() < limit,
                    "PalmDOC expansion exceeds the text limit"
                );
                output.push(byte);
            }
            128..=191 => {
                let next = *bytes.get(at).context("Truncated PalmDOC back-reference")?;
                at += 1;
                let code = (((byte as usize) << 8) | next as usize) & 0x3fff;
                let distance = code >> 3;
                let length = (code & 7) + 3;
                ensure!(
                    distance > 0 && distance <= output.len(),
                    "Invalid PalmDOC back-reference distance"
                );
                ensure!(
                    length <= limit && output.len() <= limit - length,
                    "PalmDOC expansion exceeds the text limit"
                );
                for _ in 0..length {
                    output.push(output[output.len() - distance]);
                }
            }
            _ => {
                ensure!(
                    limit >= 2 && output.len() <= limit - 2,
                    "PalmDOC expansion exceeds the text limit"
                );
                output.extend_from_slice(&[b' ', byte ^ 0x80]);
            }
        }
    }
    check_cancel(cancel)?;
    Ok(output)
}

#[derive(Clone, Copy, Default)]
struct Code {
    bits: usize,
    terminal: bool,
    maximum: u64,
}
struct Phrase<'a> {
    bytes: &'a [u8],
    literal: bool,
    cached: Option<Vec<u8>>,
    active: bool,
}
struct Huff<'a> {
    table: [Code; 256],
    minimum: [u64; 33],
    maximum: [u64; 33],
    phrases: Vec<Phrase<'a>>,
    cache_bytes: usize,
    work: usize,
}
impl<'a> Huff<'a> {
    fn new(records: &[&'a [u8]], cancel: &AtomicBool) -> Result<Self> {
        let huff = *records.first().context("HUFF table is missing")?;
        ensure!(
            slice(huff, 0, 4)? == b"HUFF" && be32(huff, 4)? == 24,
            "Invalid HUFF dictionary header"
        );
        let table_offset = be32(huff, 8)? as usize;
        let ranges_offset = be32(huff, 12)? as usize;
        slice(huff, table_offset, 1024)?;
        slice(huff, ranges_offset, 256)?;
        let mut decoder = Self {
            table: [Code::default(); 256],
            minimum: [0; 33],
            maximum: [0; 33],
            phrases: Vec::new(),
            cache_bytes: 0,
            work: 0,
        };
        for index in 0..256 {
            let entry = be32(huff, table_offset + index * 4)?;
            let bits = (entry & 31) as usize;
            let terminal = entry & 0x80 != 0;
            ensure!(
                bits > 0 && (bits > 8 || terminal),
                "Invalid HUFF code length or terminal flag"
            );
            let maximum = (((entry >> 8) as u64 + 1) << (32 - bits))
                .checked_sub(1)
                .context("Invalid HUFF maximum code")?;
            ensure!(
                !terminal || maximum <= u32::MAX as u64,
                "HUFF maximum code overflows"
            );
            decoder.table[index] = Code {
                bits,
                terminal,
                maximum,
            };
        }
        for bits in 1..=32 {
            let minimum = (be32(huff, ranges_offset + (bits - 1) * 8)? as u64) << (32 - bits);
            let maximum =
                (((be32(huff, ranges_offset + (bits - 1) * 8 + 4)? as u64) + 1) << (32 - bits)) - 1;
            // An unused code length can have a minimum of 2^32, one past
            // every possible lookahead value. Retain that sentinel in u64.
            ensure!(minimum <= (1u64 << 32), "Invalid HUFF minimum code range");
            decoder.minimum[bits] = minimum;
            decoder.maximum[bits] = maximum;
        }
        let mut expected = None;
        for cdic in records.iter().skip(1) {
            check_cancel(cancel)?;
            ensure!(
                slice(cdic, 0, 4)? == b"CDIC" && be32(cdic, 4)? == 16,
                "Invalid CDIC dictionary header"
            );
            let total = be32(cdic, 8)? as usize;
            ensure!(
                total > 0 && total <= MAX_PHRASES && expected.is_none_or(|old| old == total),
                "Invalid or oversized CDIC phrase count"
            );
            expected = Some(total);
            let bits = be32(cdic, 12)? as usize;
            ensure!(
                bits <= 16,
                "CDIC phrase index width exceeds the bounded decoder limit"
            );
            ensure!(
                decoder.phrases.len() < total,
                "Unexpected extra CDIC dictionary record"
            );
            let count = (1usize << bits).min(total - decoder.phrases.len());
            slice(cdic, 16, count * 2)?;
            for index in 0..count {
                let at = 16 + be16(cdic, 16 + index * 2)? as usize;
                ensure!(
                    at >= 16 + count * 2,
                    "CDIC phrase overlaps its offset table"
                );
                let size = be16(cdic, at)?;
                let data = slice(cdic, at + 2, (size & 0x7fff) as usize)?;
                decoder.phrases.push(Phrase {
                    bytes: data,
                    literal: size & 0x8000 != 0,
                    cached: None,
                    active: false,
                });
            }
        }
        ensure!(
            expected == Some(decoder.phrases.len()),
            "CDIC dictionaries contain missing phrases"
        );
        Ok(decoder)
    }

    fn unpack(
        &mut self,
        bytes: &[u8],
        limit: usize,
        depth: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>> {
        ensure!(
            depth <= 32,
            "HUFF dictionary nesting exceeds the depth limit"
        );
        let mut output = Vec::new();
        let total_bits = bytes
            .len()
            .checked_mul(8)
            .context("HUFF bit length overflow")?;
        let mut position = 0;
        while position < total_bits {
            self.work = self
                .work
                .checked_add(1)
                .context("HUFF work counter overflow")?;
            ensure!(
                self.work <= MAX_TEXT * 8,
                "HUFF decoding exceeds the bounded work limit"
            );
            if self.work.is_multiple_of(1024) {
                check_cancel(cancel)?;
            }
            // Read at most five bytes into a padded 32-bit lookahead window.
            let mut word = 0u64;
            let byte_at = position / 8;
            for index in 0..5 {
                word = (word << 8) | bytes.get(byte_at + index).copied().unwrap_or(0) as u64;
            }
            let code = ((word << (position % 8)) >> 8) as u32;
            let entry = self.table[(code >> 24) as usize];
            let mut bits = entry.bits;
            let mut maximum = entry.maximum;
            if !entry.terminal {
                while bits <= 32 && (code as u64) < self.minimum[bits] {
                    bits += 1;
                }
                ensure!(bits <= 32, "HUFF code has no matching range");
                maximum = self.maximum[bits];
            }
            if bits > total_bits - position {
                break;
            } // final zero padding has no complete symbol
            ensure!(
                maximum <= u32::MAX as u64 && (code as u64) <= maximum,
                "Invalid HUFF code value"
            );
            let index = ((maximum - code as u64) >> (32 - bits)) as usize;
            ensure!(
                index < self.phrases.len(),
                "HUFF code references a missing CDIC phrase"
            );
            position += bits;
            if !self.phrases[index].literal && self.phrases[index].cached.is_none() {
                ensure!(
                    !self.phrases[index].active,
                    "HUFF dictionary contains a reference cycle"
                );
                self.phrases[index].active = true;
                let compressed = self.phrases[index].bytes;
                let expanded = self.unpack(compressed, MAX_PHRASE, depth + 1, cancel)?;
                self.phrases[index].active = false;
                self.cache_bytes = self
                    .cache_bytes
                    .checked_add(expanded.len())
                    .context("HUFF cache size overflow")?;
                ensure!(
                    self.cache_bytes <= MAX_DICT_CACHE,
                    "HUFF dictionary cache exceeds the 64 MiB limit"
                );
                self.phrases[index].cached = Some(expanded);
            }
            let phrase = &self.phrases[index];
            let data = phrase.cached.as_deref().unwrap_or(phrase.bytes);
            ensure!(
                data.len() <= limit && output.len() <= limit - data.len(),
                "HUFF expansion exceeds the text limit"
            );
            output.extend_from_slice(data);
        }
        check_cancel(cancel)?;
        Ok(output)
    }
}

/// Original synthetic MOBI6: two HTML headings/pagebreak, numeric filepos link,
/// one PNG recindex=1 cover, title Native MOBI Fixture, author Mobi Reader Test Studio.
#[cfg(test)]
pub(crate) fn fixture() -> Vec<u8> {
    tests::fixture_kind(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    fn set16(bytes: &mut [u8], at: usize, value: u16) {
        bytes[at..at + 2].copy_from_slice(&value.to_be_bytes());
    }
    fn set32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_be_bytes());
    }
    fn pdb(records: &[Vec<u8>]) -> Vec<u8> {
        let base = 78 + records.len() * 8 + 2;
        let mut bytes = vec![0; base];
        bytes[..12].copy_from_slice(b"MOBI fixture");
        bytes[60..68].copy_from_slice(b"BOOKMOBI");
        set16(&mut bytes, 76, records.len() as u16);
        for (index, record) in records.iter().enumerate() {
            let offset = bytes.len() as u32;
            set32(&mut bytes, 78 + index * 8, offset);
            bytes.extend_from_slice(record);
        }
        bytes
    }
    fn metadata() -> Vec<u8> {
        let mut bytes = b"EXTH".to_vec();
        bytes.extend_from_slice(&[0; 8]);
        for (kind, value) in [
            (100u32, b"Mobi Reader Test Studio".as_slice()),
            (201, 0u32.to_be_bytes().as_slice()),
            (503, b"Native MOBI Fixture".as_slice()),
        ] {
            bytes.extend_from_slice(&kind.to_be_bytes());
            bytes.extend_from_slice(&(value.len() as u32 + 8).to_be_bytes());
            bytes.extend_from_slice(value);
        }
        let length = bytes.len() as u32;
        set32(&mut bytes, 4, length);
        set32(&mut bytes, 8, 3);
        bytes
    }
    fn huff_fixture() -> (Vec<u8>, Vec<u8>) {
        let mut huff = vec![0; 24 + 1024 + 256];
        huff[..4].copy_from_slice(b"HUFF");
        set32(&mut huff, 4, 24);
        set32(&mut huff, 8, 24);
        set32(&mut huff, 12, 1048);
        for index in 0..256 {
            set32(&mut huff, 24 + index * 4, (255 << 8) | 0x88);
        }
        for bits in 1..=32 {
            set32(
                &mut huff,
                1048 + (bits - 1) * 8 + 4,
                ((1u64 << bits) - 1) as u32,
            );
        }
        let mut cdic = vec![0; 16 + 512];
        cdic[..4].copy_from_slice(b"CDIC");
        set32(&mut cdic, 4, 16);
        set32(&mut cdic, 8, 256);
        set32(&mut cdic, 12, 8);
        for index in 0..256 {
            let offset = cdic.len() - 16;
            set16(&mut cdic, 16 + index * 2, offset as u16);
            cdic.extend_from_slice(&0x8001u16.to_be_bytes());
            cdic.push((255 - index) as u8);
        }
        (huff, cdic)
    }
    pub(super) fn fixture_kind(compression: u16) -> Vec<u8> {
        let mut html = "<html><body><h1 id='opening'>Opening</h1><p>A native MOBI book with café and ไทย.</p><p><a filepos='0000000000'>Continue</a><img recindex='1' alt='Cover'/></p><mbp:pagebreak/><h1 id='second'>Second chapter</h1><p>This chapter has a second passage.</p></body></html>".as_bytes().to_vec();
        let target = std::str::from_utf8(&html)
            .unwrap()
            .find("<h1 id='second'>")
            .unwrap();
        let placeholder = std::str::from_utf8(&html)
            .unwrap()
            .find("0000000000")
            .unwrap();
        html[placeholder..placeholder + 10].copy_from_slice(format!("{target:010}").as_bytes());
        let mut head = vec![0; 248];
        set16(&mut head, 0, compression);
        set32(&mut head, 4, html.len() as u32);
        set16(&mut head, 8, 1);
        set16(&mut head, 10, 4096);
        head[16..20].copy_from_slice(b"MOBI");
        set32(&mut head, 20, 232);
        set32(&mut head, 24, 2);
        set32(&mut head, 28, 65001);
        set32(&mut head, 36, 6);
        set32(&mut head, 128, 0x40);
        set32(&mut head, 168, u32::MAX);
        head.extend_from_slice(&metadata());
        let name = b"Native MOBI Fixture";
        let name_at = head.len() as u32;
        set32(&mut head, 84, name_at);
        set32(&mut head, 88, name.len() as u32);
        head.extend_from_slice(name);
        let compressed = if compression == 2 {
            let mut value = Vec::new();
            for byte in &html {
                if matches!(*byte, 0 | 9..=127) {
                    value.push(*byte);
                } else {
                    value.extend_from_slice(&[1, *byte]);
                }
            }
            value
        } else {
            html
        };
        let mut records = vec![head, compressed];
        if compression == 17480 {
            let (huff, cdic) = huff_fixture();
            set32(&mut records[0], 112, 2);
            set32(&mut records[0], 116, 2);
            records.push(huff);
            records.push(cdic);
        }
        let first_image = records.len() as u32;
        set32(&mut records[0], 108, first_image);
        records.push(STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=").unwrap());
        pdb(&records)
    }
    #[test]
    fn parses_original_mobi_none_palmdoc_and_huff_cdic() {
        for compression in [1, 2, 17480] {
            let result = decode_bytes(&fixture_kind(compression), &AtomicBool::new(false)).unwrap();
            assert_eq!(result.title, "Native MOBI Fixture");
            assert_eq!(result.authors, vec!["Mobi Reader Test Studio"]);
            assert_eq!(result.encoding, 65001);
            assert_eq!(result.cover, Some(1));
            assert_eq!(result.images[0].recindex, 1);
            assert!(
                std::str::from_utf8(&result.text)
                    .unwrap()
                    .contains("café and ไทย")
            );
            assert!(
                result
                    .text
                    .windows(b"<mbp:pagebreak/>".len())
                    .any(|part| part == b"<mbp:pagebreak/>")
            );
        }
    }
    #[test]
    fn rejects_damaged_encrypted_kf8_and_cancelled() {
        let cancel = AtomicBool::new(false);
        for length in [0, 60, 78, 100] {
            assert!(decode_bytes(&fixture()[..length], &cancel).is_err());
        }
        let mut damaged = fixture();
        set32(&mut damaged, 78, u32::MAX);
        assert!(decode_bytes(&damaged, &cancel).is_err());
        let mut encrypted = fixture();
        let at = be32(&encrypted, 78).unwrap() as usize;
        set16(&mut encrypted, at + 12, 2);
        assert!(
            decode_bytes(&encrypted, &cancel)
                .unwrap_err()
                .to_string()
                .contains("Encrypted")
        );
        let mut kf8 = fixture();
        let at = be32(&kf8, 78).unwrap() as usize;
        set32(&mut kf8, at + 36, 8);
        assert!(
            decode_bytes(&kf8, &cancel)
                .unwrap_err()
                .to_string()
                .contains("KF8")
        );
        assert!(decode_bytes(&fixture(), &AtomicBool::new(true)).is_err());
    }

    #[test]
    fn palmdoc_saved_position_is_not_mobi_encryption() {
        let mut header = vec![0; 16];
        set16(&mut header, 0, 1);
        set32(&mut header, 4, 1);
        set16(&mut header, 8, 1);
        set16(&mut header, 10, 4096);
        set32(&mut header, 12, 0x12345678);
        let mut bytes = pdb(&[header, b"x".to_vec()]);
        bytes[60..68].copy_from_slice(b"TEXtREAd");
        let decoded = decode_bytes(&bytes, &AtomicBool::new(false)).unwrap();
        assert_eq!(decoded.text, b"x");
        assert_eq!(decoded.encoding, 1252);
    }

    #[test]
    fn dual_mobi_uses_legacy_text_and_windows1252_metadata_decodes() {
        let original = fixture();
        let count = be16(&original, 76).unwrap() as usize;
        let mut records = Vec::new();
        for index in 0..count {
            let start = be32(&original, 78 + index * 8).unwrap() as usize;
            let end = if index + 1 == count {
                original.len()
            } else {
                be32(&original, 78 + (index + 1) * 8).unwrap() as usize
            };
            records.push(original[start..end].to_vec());
        }
        let boundary = records.len();
        let exth = 248;
        let length = be32(&records[0], exth + 4).unwrap() as usize;
        let title_at = be32(&records[0], 84).unwrap();
        let mut entry = 121u32.to_be_bytes().to_vec();
        entry.extend_from_slice(&12u32.to_be_bytes());
        entry.extend_from_slice(&((boundary + 1) as u32).to_be_bytes());
        records[0].splice(exth + length..exth + length, entry);
        set32(&mut records[0], exth + 4, (length + 12) as u32);
        set32(&mut records[0], exth + 8, 4);
        set32(&mut records[0], 84, title_at + 12);
        records.push(b"BOUNDARY".to_vec());
        let mut kf8 = records[0].clone();
        set32(&mut kf8, 36, 8);
        records.push(kf8);
        records.push(b"KF8-only payload must not enter legacy text".to_vec());
        let result = decode_bytes(&pdb(&records), &AtomicBool::new(false)).unwrap();
        assert_eq!(
            result.text,
            decode_bytes(&original, &AtomicBool::new(false))
                .unwrap()
                .text
        );
        assert_eq!(
            text_string(b"Caf\xe9 \x93quiet\x94", 1252).unwrap(),
            "Café “quiet”"
        );
    }
    #[test]
    fn palmdoc_validates_backreferences_literals_and_output_bounds() {
        let cancel = AtomicBool::new(false);
        assert_eq!(palmdoc(b"abc\x80\x18", 6, &cancel).unwrap(), b"abcabc");
        assert_eq!(palmdoc(b"a\x80\x0b", 7, &cancel).unwrap(), b"aaaaaaa");
        assert_eq!(palmdoc(&[0xe1], 2, &cancel).unwrap(), b" a");
        for value in [b"\x80\x18".as_slice(), b"\x80", b"\x03x", b"abc\x80\x18"] {
            assert!(palmdoc(value, 3, &cancel).is_err());
        }
    }
    #[test]
    fn handles_trailing_data_without_changing_filepos_bytes() {
        assert_eq!(strip_trailing(b"text\x80\x02", 2).unwrap(), b"text");
        assert_eq!(strip_trailing(b"text\x81", 2).unwrap(), b"text");
        assert_eq!(strip_trailing(b"text\x00", 1).unwrap(), b"text");
        assert!(strip_trailing(b"\x7f", 2).is_err());
        assert!(strip_trailing(b"\x83", 1).is_err());
    }
    #[test]
    fn huff_rejects_cycles_missing_phrases_and_expansion_bombs() {
        let cancel = AtomicBool::new(false);
        let (huff, mut cdic) = huff_fixture();
        let at = 16 + be16(&cdic, 16 + 255 * 2).unwrap() as usize;
        set16(&mut cdic, at, 1);
        cdic[at + 2] = 0;
        let mut decoder = Huff::new(&[&huff, &cdic], &cancel).unwrap();
        assert!(
            decoder
                .unpack(&[0], 20, 0, &cancel)
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );
        let (huff, cdic) = huff_fixture();
        let mut decoder = Huff::new(&[&huff, &cdic], &cancel).unwrap();
        assert!(decoder.unpack(b"long", 2, 0, &cancel).is_err());
        assert!(Huff::new(&[&huff, &cdic[..100]], &cancel).is_err());
        assert!(Huff::new(&[], &cancel).is_err());
    }

    #[test]
    fn huff_decodes_nonterminal_ranges_and_acyclic_nested_phrases() {
        let cancel = AtomicBool::new(false);
        let (mut huff, mut cdic) = huff_fixture();
        // Prefix 255 requires a second-stage lookup: 511 uses nine bits,
        // while 1020/1021 use ten. Other prefixes retain eight-bit symbols.
        set32(&mut huff, 24 + 255 * 4, (511 << 8) | 9);
        for bits in 1..32 {
            set32(&mut huff, 1048 + (bits - 1) * 8, 1u32 << bits);
            set32(&mut huff, 1048 + (bits - 1) * 8 + 4, 0);
        }
        set32(&mut huff, 1048 + 31 * 8, u32::MAX);
        set32(&mut huff, 1048 + 8 * 8, 511);
        set32(&mut huff, 1048 + 8 * 8 + 4, 511);
        set32(&mut huff, 1048 + 9 * 8, 1020);
        set32(&mut huff, 1048 + 9 * 8 + 4, 1021);
        // Phrase zero is itself compressed: the eight-bit 'a' symbol points
        // to an independent literal phrase. Phrase one is literal 'B'.
        let zero = 16 + be16(&cdic, 16).unwrap() as usize;
        set16(&mut cdic, zero, 1);
        cdic[zero + 2] = b'a';
        let one = 16 + be16(&cdic, 18).unwrap() as usize;
        cdic[one + 2] = b'B';
        let mut encoded = Vec::<u8>::new();
        let mut position = 0usize;
        for (code, width) in [(1020u32, 10usize), (511, 9), (b'Z' as u32, 8)] {
            for bit in (0..width).rev() {
                if position.is_multiple_of(8) {
                    encoded.push(0);
                }
                *encoded.last_mut().unwrap() |= (((code >> bit) & 1) as u8) << (7 - position % 8);
                position += 1;
            }
        }
        let mut decoder = Huff::new(&[&huff, &cdic], &cancel).unwrap();
        assert_eq!(decoder.unpack(&encoded, 3, 0, &cancel).unwrap(), b"BaZ");
        assert_eq!(decoder.phrases[0].cached.as_deref(), Some(b"a".as_slice()));
        assert_eq!(decoder.cache_bytes, 1);
        assert_eq!(decoder.unpack(&[0xff, 0x80], 1, 0, &cancel).unwrap(), b"a");
    }
}
