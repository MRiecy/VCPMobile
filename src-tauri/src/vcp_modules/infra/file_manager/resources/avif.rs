//! Bounded container metadata for the Android static-AVIF display fallback.
//! This does not decode or validate AV1 payloads, and the encoded-byte/pixel
//! budgets are not a promise about the browser decoder's total memory usage.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

const MAX_META_BYTES: u64 = 1024 * 1024;
const MAX_BOXES: usize = 4096;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Metadata {
    pub size: u64,
    pub width: u32,
    pub height: u32,
}

type ProbeResult<T> = Result<T, String>;

fn invalid(reason: &str) -> String {
    format!("AVIF display metadata: {reason}")
}

fn count_box(remaining: &mut usize) -> ProbeResult<()> {
    *remaining = remaining
        .checked_sub(1)
        .ok_or_else(|| invalid("too many boxes"))?;
    Ok(())
}

struct FileBox {
    kind: [u8; 4],
    body: u64,
    end: u64,
}

fn file_box(
    reader: &mut (impl Read + Seek),
    start: u64,
    parent_end: u64,
    remaining: &mut usize,
) -> ProbeResult<FileBox> {
    count_box(remaining)?;
    if parent_end.saturating_sub(start) < 8 {
        return Err(invalid("truncated box header"));
    }
    reader
        .seek(SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    let mut header = [0; 8];
    reader.read_exact(&mut header).map_err(|e| e.to_string())?;
    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    let kind = [header[4], header[5], header[6], header[7]];
    let (length, header_size) = match length {
        0 => (parent_end - start, 8),
        1 => {
            if parent_end - start < 16 {
                return Err(invalid("truncated extended box header"));
            }
            let mut extended = [0; 8];
            reader
                .read_exact(&mut extended)
                .map_err(|e| e.to_string())?;
            (u64::from_be_bytes(extended), 16)
        }
        value => (u64::from(value), 8),
    };
    let end = start
        .checked_add(length)
        .filter(|end| *end <= parent_end)
        .ok_or_else(|| invalid("box extends beyond its parent"))?;
    if length < header_size {
        return Err(invalid("box is smaller than its header"));
    }
    let body = start
        .checked_add(header_size)
        .ok_or_else(|| invalid("box offset overflow"))?;
    Ok(FileBox { kind, body, end })
}

fn static_avif_brand(reader: &mut impl Read, bytes: u64) -> ProbeResult<()> {
    if bytes < 8 || bytes % 4 != 0 {
        return Err(invalid("invalid ftyp length"));
    }
    let mut brand = [0; 4];
    reader.read_exact(&mut brand).map_err(|e| e.to_string())?;
    let mut avif = brand == *b"avif";
    let mut sequence = brand == *b"avis";
    // minor_version is an integer, not a compatible brand.
    reader.read_exact(&mut brand).map_err(|e| e.to_string())?;
    for _ in 0..(bytes - 8) / 4 {
        reader.read_exact(&mut brand).map_err(|e| e.to_string())?;
        avif |= brand == *b"avif";
        sequence |= brand == *b"avis";
    }
    if !avif || sequence {
        return Err(invalid("only static avif branding is supported"));
    }
    Ok(())
}

pub(super) fn probe(source: &Path, max_bytes: u64, max_pixels: u64) -> ProbeResult<Metadata> {
    let file = File::open(source).map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    let size = metadata.len();
    if !metadata.is_file() || size == 0 || size > max_bytes {
        return Err(invalid("source exceeds the encoded file budget"));
    }
    let mut reader = BufReader::new(file);
    let mut remaining = MAX_BOXES;
    let mut offset = 0;
    let mut branded = false;
    let mut dimensions = None;
    while offset < size {
        let item = file_box(&mut reader, offset, size, &mut remaining)?;
        if offset == 0 && item.kind != *b"ftyp" {
            return Err(invalid("ftyp must be the first box"));
        }
        match &item.kind {
            b"ftyp" => {
                if branded {
                    return Err(invalid("duplicate ftyp"));
                }
                static_avif_brand(&mut reader, item.end - item.body)?;
                branded = true;
            }
            b"meta" => {
                if dimensions.is_some() || item.end - offset > MAX_META_BYTES {
                    return Err(invalid("duplicate or oversized meta"));
                }
                let length = usize::try_from(item.end - item.body)
                    .map_err(|_| invalid("metadata size overflow"))?;
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).map_err(|e| e.to_string())?;
                dimensions = Some(parse_meta(&bytes, &mut remaining, max_pixels)?);
            }
            b"moov" => return Err(invalid("image sequences are not supported")),
            // In particular, mdat is skipped by seeking to the next top-level
            // header. Its declared size is checked, but its payload is not read.
            _ => {}
        }
        offset = item.end;
    }
    if !branded
        || reader
            .get_ref()
            .metadata()
            .map_err(|e| e.to_string())?
            .len()
            != size
    {
        return Err(invalid("file changed during metadata inspection"));
    }
    let (width, height) = dimensions.ok_or_else(|| invalid("missing meta"))?;
    Ok(Metadata {
        size,
        width,
        height,
    })
}

fn take<'a>(bytes: &mut &'a [u8], length: usize) -> ProbeResult<&'a [u8]> {
    let (value, rest) = bytes
        .split_at_checked(length)
        .ok_or_else(|| invalid("truncated metadata field"))?;
    *bytes = rest;
    Ok(value)
}

fn byte(bytes: &mut &[u8]) -> ProbeResult<u8> {
    Ok(take(bytes, 1)?[0])
}

fn word(bytes: &mut &[u8]) -> ProbeResult<u16> {
    let value = take(bytes, 2)?;
    Ok(u16::from_be_bytes([value[0], value[1]]))
}

fn dword(bytes: &mut &[u8]) -> ProbeResult<u32> {
    let value = take(bytes, 4)?;
    Ok(u32::from_be_bytes([value[0], value[1], value[2], value[3]]))
}

fn full_box(bytes: &mut &[u8]) -> ProbeResult<(u8, u32)> {
    let value = dword(bytes)?;
    Ok(((value >> 24) as u8, value & 0x00ff_ffff))
}

struct SliceBox<'a> {
    kind: [u8; 4],
    body: &'a [u8],
}

fn next_box<'a>(bytes: &mut &'a [u8], remaining: &mut usize) -> ProbeResult<Option<SliceBox<'a>>> {
    if bytes.is_empty() {
        return Ok(None);
    }
    count_box(remaining)?;
    let parent_length = bytes.len();
    let size = dword(bytes)?;
    let kind = take(bytes, 4)?;
    let kind = [kind[0], kind[1], kind[2], kind[3]];
    let (size, header) = match size {
        0 => (parent_length as u64, 8),
        1 => {
            let value = take(bytes, 8)?;
            let size = u64::from_be_bytes([
                value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
            ]);
            (size, 16)
        }
        value => (u64::from(value), 8),
    };
    let length = size
        .checked_sub(header)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| invalid("invalid metadata box length"))?;
    Ok(Some(SliceBox {
        kind,
        body: take(bytes, length)?,
    }))
}

fn parse_primary(mut bytes: &[u8]) -> ProbeResult<u32> {
    let (version, flags) = full_box(&mut bytes)?;
    if flags != 0 {
        return Err(invalid("unsupported pitm flags"));
    }
    let id = match version {
        0 => u32::from(word(&mut bytes)?),
        1 => dword(&mut bytes)?,
        _ => return Err(invalid("unsupported pitm version")),
    };
    if id == 0 || !bytes.is_empty() {
        return Err(invalid("invalid primary item"));
    }
    Ok(id)
}

fn parse_meta(mut bytes: &[u8], remaining: &mut usize, max_pixels: u64) -> ProbeResult<(u32, u32)> {
    if full_box(&mut bytes)? != (0, 0) {
        return Err(invalid("unsupported meta version or flags"));
    }
    let mut primary = None;
    let mut properties = None;
    while let Some(item) = next_box(&mut bytes, remaining)? {
        match &item.kind {
            b"pitm" => {
                if primary.replace(parse_primary(item.body)?).is_some() {
                    return Err(invalid("duplicate pitm"));
                }
            }
            b"iprp" => {
                if properties.replace(item.body).is_some() {
                    return Err(invalid("duplicate iprp"));
                }
            }
            _ => {}
        }
    }
    parse_properties(
        properties.ok_or_else(|| invalid("missing iprp"))?,
        primary.ok_or_else(|| invalid("missing pitm"))?,
        remaining,
        max_pixels,
    )
}

#[derive(Clone, Copy)]
enum Property {
    Dimensions(u32, u32),
    Rotation(u8),
    Crop,
    Other,
}

fn dimensions(mut bytes: &[u8], max_pixels: u64) -> ProbeResult<Property> {
    if full_box(&mut bytes)? != (0, 0) {
        return Err(invalid("unsupported ispe version or flags"));
    }
    let width = dword(&mut bytes)?;
    let height = dword(&mut bytes)?;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| invalid("pixel count overflow"))?;
    if width == 0 || height == 0 || pixels > max_pixels || !bytes.is_empty() {
        return Err(invalid("invalid or oversized spatial extents"));
    }
    Ok(Property::Dimensions(width, height))
}

fn parse_properties(
    mut bytes: &[u8],
    primary: u32,
    remaining: &mut usize,
    max_pixels: u64,
) -> ProbeResult<(u32, u32)> {
    let mut container = None;
    let mut association_boxes = Vec::new();
    while let Some(item) = next_box(&mut bytes, remaining)? {
        match &item.kind {
            b"ipco" => {
                if container.replace(item.body).is_some() {
                    return Err(invalid("duplicate ipco"));
                }
            }
            b"ipma" => association_boxes.push(item.body),
            _ => {}
        }
    }
    let mut bytes = container.ok_or_else(|| invalid("missing ipco"))?;
    let mut properties = Vec::new();
    while let Some(item) = next_box(&mut bytes, remaining)? {
        // Preserve every property's 1-based index, including unneeded types.
        properties.push(match &item.kind {
            b"ispe" => dimensions(item.body, max_pixels)?,
            b"irot" => {
                if item.body.len() != 1 || item.body[0] > 3 {
                    return Err(invalid("invalid irot"));
                }
                Property::Rotation(item.body[0])
            }
            b"clap" => Property::Crop,
            _ => Property::Other,
        });
    }
    let mut size = None;
    let mut rotation = None;
    let mut primary_entries = 0;
    for mut bytes in association_boxes {
        let (version, flags) = full_box(&mut bytes)?;
        if version > 1 || flags & !1 != 0 {
            return Err(invalid("unsupported ipma version or flags"));
        }
        let entries = dword(&mut bytes)?;
        // Each entry consumes at least an item ID and association count. This
        // check bounds the loop before trusting its declared entry count.
        let minimum_entry_bytes = if version == 0 { 3 } else { 5 };
        if u64::from(entries) > (bytes.len() / minimum_entry_bytes) as u64 {
            return Err(invalid("truncated ipma entries"));
        }
        for _ in 0..entries {
            let id = if version == 0 {
                u32::from(word(&mut bytes)?)
            } else {
                dword(&mut bytes)?
            };
            let count = byte(&mut bytes)?;
            if id == primary {
                primary_entries += 1;
                if primary_entries > 1 {
                    return Err(invalid("ambiguous primary associations"));
                }
            }
            for _ in 0..count {
                let (index, essential) = if flags & 1 == 0 {
                    let value = byte(&mut bytes)?;
                    (usize::from(value & 0x7f), value & 0x80 != 0)
                } else {
                    let value = word(&mut bytes)?;
                    (usize::from(value & 0x7fff), value & 0x8000 != 0)
                };
                if index == 0 {
                    if essential {
                        return Err(invalid("essential property has index zero"));
                    }
                    continue;
                }
                let property = properties
                    .get(index - 1)
                    .ok_or_else(|| invalid("property index outside ipco"))?;
                if id != primary {
                    continue;
                }
                match *property {
                    Property::Dimensions(width, height) => {
                        if size.replace((width, height)).is_some() {
                            return Err(invalid("multiple primary spatial extents"));
                        }
                    }
                    Property::Rotation(angle) => {
                        if rotation.replace(angle).is_some() {
                            return Err(invalid("multiple primary rotations"));
                        }
                    }
                    Property::Crop => {
                        return Err(invalid(
                            "primary crop is not supported by the display fallback",
                        ))
                    }
                    Property::Other => {}
                }
            }
        }
        if !bytes.is_empty() {
            return Err(invalid("trailing ipma data"));
        }
    }
    let (width, height) = size.ok_or_else(|| invalid("primary item has no spatial extents"))?;
    Ok(if rotation.is_some_and(|angle| angle % 2 == 1) {
        (height, width)
    } else {
        (width, height)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(payload);
        bytes
    }

    fn full(kind: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
        let mut bytes = ((u32::from(version) << 24) | flags).to_be_bytes().to_vec();
        bytes.extend_from_slice(payload);
        boxed(kind, &bytes)
    }

    fn spatial(width: u32, height: u32) -> Vec<u8> {
        full(
            b"ispe",
            0,
            0,
            &[width.to_be_bytes(), height.to_be_bytes()].concat(),
        )
    }

    fn associations(version: u8, wide: bool, entries: &[(u32, &[u16])]) -> Vec<u8> {
        let mut bytes = (entries.len() as u32).to_be_bytes().to_vec();
        for &(id, properties) in entries {
            if version == 0 {
                bytes.extend_from_slice(&(id as u16).to_be_bytes());
            } else {
                bytes.extend_from_slice(&id.to_be_bytes());
            }
            bytes.push(properties.len() as u8);
            for &property in properties {
                if wide {
                    bytes.extend_from_slice(&property.to_be_bytes());
                } else {
                    bytes.push(property as u8);
                }
            }
        }
        full(b"ipma", version, u32::from(wide), &bytes)
    }

    fn metadata(primary: u32, version: u8, properties: &[u8], associations: &[u8]) -> Vec<u8> {
        let id = if version == 0 {
            (primary as u16).to_be_bytes().to_vec()
        } else {
            primary.to_be_bytes().to_vec()
        };
        // Deliberately put ipma before ipco and pitm after iprp.
        let iprp = boxed(
            b"iprp",
            &[associations, &boxed(b"ipco", properties)].concat(),
        );
        full(
            b"meta",
            0,
            0,
            &[iprp, full(b"pitm", version, 0, &id)].concat(),
        )
    }

    fn fixture(meta: &[u8]) -> Vec<u8> {
        [
            boxed(b"ftyp", b"avif\0\0\0\0mif1"),
            boxed(b"mdat", &[42; 16]),
            meta.to_vec(),
        ]
        .concat()
    }

    fn check_file(bytes: &[u8], max_bytes: u64, max_pixels: u64) -> ProbeResult<Metadata> {
        // The fixture lives under a repository-relative temporary directory.
        let root = Path::new("target").join("avif-metadata-tests");
        std::fs::create_dir_all(&root).expect("fixture root");
        let path = root.join(format!("{}.avif", uuid::Uuid::new_v4()));
        std::fs::write(&path, bytes).expect("fixture");
        let result = probe(&path, max_bytes, max_pixels);
        std::fs::remove_file(path).expect("fixture cleanup");
        result
    }

    fn parse(bytes: &[u8], max_pixels: u64) -> ProbeResult<Metadata> {
        check_file(bytes, 24 * 1024 * 1024, max_pixels)
    }

    #[test]
    fn follows_primary_associations_instead_of_first_or_largest_ispe() {
        let props = [
            spatial(32, 24),
            boxed(b"av1C", &[0; 4]),
            spatial(640, 480),
            spatial(1000, 800),
        ]
        .concat();
        let meta = metadata(
            7,
            0,
            &props,
            &associations(0, false, &[(1, &[1]), (7, &[0, 0x82, 3]), (9, &[4])]),
        );
        let bytes = fixture(&meta);
        assert_eq!(
            parse(&bytes, 1_000_000).expect("primary"),
            Metadata {
                size: bytes.len() as u64,
                width: 640,
                height: 480
            }
        );
    }

    #[test]
    fn supports_wide_item_ids_and_property_indices_and_primary_rotation() {
        let mut props = boxed(b"irot", &[2]);
        for _ in 1..128 {
            props.extend(boxed(b"free", &[]));
        }
        props.extend(spatial(640, 480));
        props.extend(boxed(b"irot", &[1]));
        let meta = metadata(
            70_000,
            1,
            &props,
            &associations(1, true, &[(2, &[1]), (70_000, &[0x8000 | 129, 130])]),
        );
        let result = parse(&fixture(&meta), 640 * 480).expect("wide properties");
        assert_eq!((result.width, result.height), (480, 640));
    }

    #[test]
    fn enforces_file_pixel_metadata_and_box_count_budgets() {
        let meta = metadata(
            1,
            0,
            &spatial(2048, 2048),
            &associations(0, false, &[(1, &[1])]),
        );
        let bytes = fixture(&meta);
        assert!(parse(&bytes, 4 * 1024 * 1024).is_ok());
        assert!(parse(&bytes, 4 * 1024 * 1024 - 1).is_err());
        assert!(check_file(&bytes, bytes.len() as u64 - 1, u64::MAX).is_err());
        let oversized = fixture(&full(b"meta", 0, 0, &vec![0; MAX_META_BYTES as usize]));
        assert!(parse(&oversized, u64::MAX).is_err());
        let mut many = bytes;
        for _ in 0..MAX_BOXES {
            many.extend(boxed(b"free", &[]));
        }
        assert!(parse(&many, u64::MAX).is_err());
    }

    #[test]
    fn rejects_sequence_brands_and_movie_boxes_even_after_valid_metadata() {
        let meta = metadata(1, 0, &spatial(2, 3), &associations(0, false, &[(1, &[1])]));
        for brands in [
            b"avis\0\0\0\0avif".as_slice(),
            b"avif\0\0\0\0avis".as_slice(),
            b"mif1avifmif1".as_slice(),
        ] {
            assert!(parse(&[boxed(b"ftyp", brands), meta.clone()].concat(), 100).is_err());
        }
        let bytes = [fixture(&meta), boxed(b"moov", &[])].concat();
        assert!(parse(&bytes, 100).is_err());
    }

    #[test]
    fn supports_extended_sizes_and_size_zero_to_parent_end() {
        let spatial = spatial(12, 9);
        let mut extended = 1u32.to_be_bytes().to_vec();
        extended.extend_from_slice(b"ispe");
        extended.extend_from_slice(&(spatial.len() as u64 + 8).to_be_bytes());
        extended.extend_from_slice(&spatial[8..]);
        let mut meta = metadata(1, 0, &extended, &associations(0, false, &[(1, &[1])]));
        meta[..4].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(
            parse(&fixture(&meta), 108).expect("extended size").width,
            12
        );
    }

    #[test]
    fn rejects_truncation_parent_crossing_and_duplicate_primary_metadata() {
        let meta = metadata(1, 0, &spatial(12, 9), &associations(0, false, &[(1, &[1])]));
        let bytes = fixture(&meta);
        for cut in 0..bytes.len() {
            assert!(parse(&bytes[..cut], 1000).is_err(), "cut {cut}");
        }
        let mut bad_property = spatial(12, 9);
        bad_property[..4].copy_from_slice(&1000u32.to_be_bytes());
        assert!(parse(
            &fixture(&metadata(
                1,
                0,
                &bad_property,
                &associations(0, false, &[(1, &[1])])
            )),
            1000
        )
        .is_err());
        assert!(parse(&[bytes, meta].concat(), 1000).is_err());
        let mut overflow = boxed(b"ftyp", b"avif\0\0\0\0");
        overflow.extend_from_slice(&1u32.to_be_bytes());
        overflow.extend_from_slice(b"free");
        overflow.extend_from_slice(&u64::MAX.to_be_bytes());
        assert!(parse(&overflow, 1000).is_err());
    }

    #[test]
    fn rejects_missing_or_invalid_primary_associations_and_transform_data() {
        for links in [vec![], vec![0], vec![0x80], vec![2], vec![1, 1]] {
            let meta = metadata(
                1,
                0,
                &spatial(12, 9),
                &associations(0, false, &[(1, &links)]),
            );
            assert!(parse(&fixture(&meta), 1000).is_err(), "links {links:?}");
        }
        let duplicate = metadata(
            1,
            0,
            &spatial(12, 9),
            &associations(0, false, &[(1, &[1]), (1, &[1])]),
        );
        assert!(parse(&fixture(&duplicate), 1000).is_err());
        for transform in [boxed(b"irot", &[4]), boxed(b"clap", &[0; 32])] {
            let props = [spatial(12, 9), transform].concat();
            assert!(parse(
                &fixture(&metadata(
                    1,
                    0,
                    &props,
                    &associations(0, false, &[(1, &[1, 2])])
                )),
                1000
            )
            .is_err());
        }
    }

    #[test]
    fn rejects_unknown_required_versions_and_zero_dimensions() {
        for bad_ispe in [full(b"ispe", 1, 0, &[0; 8]), spatial(0, 1)] {
            assert!(parse(
                &fixture(&metadata(
                    1,
                    0,
                    &bad_ispe,
                    &associations(0, false, &[(1, &[1])])
                )),
                1000
            )
            .is_err());
        }
        let mut ipma = associations(0, false, &[(1, &[1])]);
        ipma[8] = 2;
        assert!(parse(&fixture(&metadata(1, 0, &spatial(1, 1), &ipma)), 1000).is_err());
    }
}
