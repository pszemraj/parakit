//! Minimal GGUF metadata reader used for startup model dtype reporting.

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

const GGUF_TYPE_UINT8: u32 = 0;
const GGUF_TYPE_INT8: u32 = 1;
const GGUF_TYPE_UINT16: u32 = 2;
const GGUF_TYPE_INT16: u32 = 3;
const GGUF_TYPE_UINT32: u32 = 4;
const GGUF_TYPE_INT32: u32 = 5;
const GGUF_TYPE_FLOAT32: u32 = 6;
const GGUF_TYPE_BOOL: u32 = 7;
const GGUF_TYPE_STRING: u32 = 8;
const GGUF_TYPE_ARRAY: u32 = 9;
const GGUF_TYPE_UINT64: u32 = 10;
const GGUF_TYPE_INT64: u32 = 11;
const GGUF_TYPE_FLOAT64: u32 = 12;
// Shared verbatim between `general.file_type` (`ggml_ftype`) and tensor
// types (`ggml_type`): codes 0-3 and 10-14 name the same quant in both enum
// spaces (see vendor/CrispASR/ggml/include/ggml.h); the 6-9 divergence is
// handled by each caller's own `extras` table below.
const COMMON_DTYPE_NAMES: &[(u32, &str)] = &[
    (0, "F32"),
    (1, "F16"),
    (2, "Q4_0"),
    (3, "Q4_1"),
    (10, "Q2_K"),
    (11, "Q3_K"),
    (12, "Q4_K"),
    (13, "Q5_K"),
    (14, "Q6_K"),
];

/// Detect the GGUF file dtype from `general.file_type` or tensor metadata.
///
/// # Returns
///
/// The detected dtype label, or `None` when the header is valid but does not
/// expose enough metadata to identify the dtype.
///
/// # Errors
///
/// Returns an error when the file cannot be read or is not a supported GGUF
/// header. Runtime callers should usually display `unknown` instead of
/// treating this as fatal.
fn detect_dtype(path: &Path) -> Result<Option<String>> {
    let mut file =
        BufReader::new(File::open(path).with_context(|| format!("open {}", path.display()))?);
    let mut magic = [0_u8; 4];
    file.read_exact(&mut magic)?;
    if &magic != b"GGUF" {
        return Err(anyhow!("not a GGUF file"));
    }

    let version = read_u32(&mut file)?;
    if !(2..=3).contains(&version) {
        return Err(anyhow!("unsupported GGUF version {version}"));
    }

    let tensor_count = read_u64(&mut file)?;
    let kv_count = read_u64(&mut file)?;

    for _ in 0..kv_count {
        let key = read_string(&mut file)?;
        let value_type = read_u32(&mut file)?;
        if key == "general.file_type" {
            return match value_type {
                // Known file-type codes are nonnegative; sign-bit values are unknown.
                GGUF_TYPE_UINT32 | GGUF_TYPE_INT32 => {
                    Ok(file_type_name(read_u32(&mut file)?).map(str::to_string))
                }
                other => {
                    skip_value(&mut file, other)?;
                    Ok(None)
                }
            };
        }
        skip_value(&mut file, value_type)?;
    }

    dominant_tensor_type(&mut file, tensor_count).map(|value| value.map(str::to_string))
}

/// Return the detected GGUF dtype, or `unknown` when detection is unavailable.
///
/// # Returns
///
/// A display-ready dtype label.
pub fn dtype_label(path: &Path) -> String {
    detect_dtype(path)
        .ok()
        .flatten()
        .unwrap_or_else(|| "unknown".to_string())
}

fn dominant_tensor_type(file: &mut impl Read, tensor_count: u64) -> Result<Option<&'static str>> {
    let mut counts: HashMap<u32, u64> = HashMap::new();
    for _ in 0..tensor_count {
        let _name = read_string(file)?;
        let dims = read_u32(file)?;
        let mut elements = 1_u64;
        for _ in 0..dims {
            elements = elements
                .checked_mul(read_u64(file)?)
                .context("GGUF tensor element count overflow")?;
        }
        let tensor_type = read_u32(file)?;
        let _offset = read_u64(file)?;
        let count = counts.entry(tensor_type).or_default();
        *count = count
            .checked_add(elements)
            .context("GGUF tensor type element count overflow")?;
    }

    // Equal element totals use the lowest type code, independent of map order.
    Ok(counts
        .into_iter()
        .max_by_key(|(tensor_type, count)| (*count, std::cmp::Reverse(*tensor_type)))
        .and_then(|(tensor_type, _)| tensor_type_name(tensor_type)))
}

fn file_type_name(file_type: u32) -> Option<&'static str> {
    dtype_name(
        file_type,
        &[
            (7, "Q8_0"),
            (8, "Q5_0"),
            (9, "Q5_1"),
            (24, "BF16"),
            (25, "MXFP4"),
            (26, "NVFP4"),
        ],
    )
}

fn tensor_type_name(tensor_type: u32) -> Option<&'static str> {
    dtype_name(tensor_type, &[(6, "Q5_0"), (7, "Q5_1"), (8, "Q8_0")])
}

fn dtype_name(code: u32, extras: &[(u32, &'static str)]) -> Option<&'static str> {
    extras
        .iter()
        .chain(COMMON_DTYPE_NAMES.iter())
        .find_map(|(candidate, name)| (*candidate == code).then_some(*name))
}

fn fixed_value_size(value_type: u32) -> Option<u64> {
    match value_type {
        GGUF_TYPE_UINT8 | GGUF_TYPE_INT8 | GGUF_TYPE_BOOL => Some(1),
        GGUF_TYPE_UINT16 | GGUF_TYPE_INT16 => Some(2),
        GGUF_TYPE_UINT32 | GGUF_TYPE_INT32 | GGUF_TYPE_FLOAT32 => Some(4),
        GGUF_TYPE_UINT64 | GGUF_TYPE_INT64 | GGUF_TYPE_FLOAT64 => Some(8),
        _ => None,
    }
}

fn skip_value(file: &mut (impl Read + Seek), value_type: u32) -> Result<()> {
    if let Some(size) = fixed_value_size(value_type) {
        return skip_bytes(file, size);
    }
    match value_type {
        GGUF_TYPE_STRING => {
            let len = read_u64(file)?;
            skip_bytes(file, len)
        }
        GGUF_TYPE_ARRAY => {
            let item_type = read_u32(file)?;
            let len = read_u64(file)?;
            if let Some(size) = fixed_value_size(item_type) {
                return skip_bytes(
                    file,
                    len.checked_mul(size)
                        .context("GGUF array byte count overflow")?,
                );
            }
            for _ in 0..len {
                skip_value(file, item_type)?;
            }
            Ok(())
        }
        other => Err(anyhow!("unknown GGUF metadata type {other}")),
    }
}

fn skip_bytes(file: &mut impl Seek, len: u64) -> Result<()> {
    let len = i64::try_from(len).context("GGUF value too large to seek past")?;
    file.seek_relative(len)?;
    Ok(())
}

fn read_string(file: &mut impl Read) -> Result<String> {
    let len = read_u64(file)?;
    if len > 1024 * 1024 {
        return Err(anyhow!("GGUF string length is too large: {len}"));
    }
    let mut buf = vec![0_u8; len as usize];
    file.read_exact(&mut buf)?;
    String::from_utf8(buf).context("GGUF string is not UTF-8")
}

fn read_u32(file: &mut impl Read) -> Result<u32> {
    let mut buf = [0_u8; 4];
    file.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(file: &mut impl Read) -> Result<u64> {
    let mut buf = [0_u8; 8];
    file.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    #[test]
    fn buffered_metadata_skips_arrays_and_preserves_following_values() {
        let mut bytes = Vec::new();
        bytes.extend(GGUF_TYPE_FLOAT32.to_le_bytes());
        bytes.extend(200_000_u64.to_le_bytes());
        bytes.resize(bytes.len() + 800_000, 0);
        bytes.extend(GGUF_TYPE_STRING.to_le_bytes());
        bytes.extend(2_u64.to_le_bytes());
        for value in ["small", "Unicode 雪"] {
            bytes.extend((value.len() as u64).to_le_bytes());
            bytes.extend(value.as_bytes());
        }
        bytes.extend(7_u32.to_le_bytes());
        let mut reader = BufReader::new(Cursor::new(bytes));
        skip_value(&mut reader, GGUF_TYPE_ARRAY).unwrap();
        skip_value(&mut reader, GGUF_TYPE_ARRAY).unwrap();
        assert_eq!(read_u32(&mut reader).unwrap(), 7);
    }

    #[test]
    fn numeric_array_skips_reject_overflow() {
        for len in [u64::MAX, i64::MAX as u64 / 8 + 1] {
            let mut bytes = Vec::new();
            bytes.extend(GGUF_TYPE_FLOAT64.to_le_bytes());
            bytes.extend(len.to_le_bytes());
            let error = skip_value(&mut Cursor::new(bytes), GGUF_TYPE_ARRAY).unwrap_err();
            assert!(error.to_string().contains("GGUF"));
        }
    }

    #[test]
    fn detects_general_file_type() {
        let dir = crate::test_support::fixture_root("gguf-tests", "general-file-type");
        let path = dir.join("q8.gguf");
        for value_type in [GGUF_TYPE_UINT32, GGUF_TYPE_INT32] {
            for (value, expected) in [(7_u32, Some("Q8_0")), (u32::MAX, None), (1 << 31, None)] {
                let mut file = File::create(&path).unwrap();
                file.write_all(b"GGUF").unwrap();
                file.write_all(&3_u32.to_le_bytes()).unwrap();
                file.write_all(&0_u64.to_le_bytes()).unwrap();
                file.write_all(&1_u64.to_le_bytes()).unwrap();
                write_string(&mut file, "general.file_type");
                file.write_all(&value_type.to_le_bytes()).unwrap();
                file.write_all(&value.to_le_bytes()).unwrap();
                drop(file);

                assert_eq!(detect_dtype(&path).unwrap().as_deref(), expected);
            }
        }
    }

    #[test]
    fn dominant_tensor_type_breaks_equal_element_counts_by_type_code() {
        for types in [[0_u32, 1_u32], [1, 0]] {
            let mut bytes = Vec::new();
            for tensor_type in types {
                bytes.extend(0_u64.to_le_bytes()); // empty tensor name
                bytes.extend(1_u32.to_le_bytes()); // one dimension
                bytes.extend(1024_u64.to_le_bytes());
                bytes.extend(tensor_type.to_le_bytes());
                bytes.extend(0_u64.to_le_bytes()); // offset
            }
            for _ in 0..32 {
                assert_eq!(
                    dominant_tensor_type(&mut Cursor::new(&bytes), 2).unwrap(),
                    Some("F32")
                );
            }
        }
    }

    #[test]
    fn small_f32_biases_do_not_hide_f16_weights_without_file_type() {
        let dir = crate::test_support::fixture_root("gguf-tests", "mixed-f16");
        let path = dir.join("mixed.gguf");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"GGUF").unwrap();
        file.write_all(&3_u32.to_le_bytes()).unwrap();
        file.write_all(&4_u64.to_le_bytes()).unwrap();
        file.write_all(&0_u64.to_le_bytes()).unwrap();
        for (name, elements, dtype) in [
            ("weight", 1024_u64, 1_u32),
            ("bias", 8, 0),
            ("norm.weight", 8, 0),
            ("norm.bias", 8, 0),
        ] {
            write_string(&mut file, name);
            file.write_all(&1_u32.to_le_bytes()).unwrap();
            file.write_all(&elements.to_le_bytes()).unwrap();
            file.write_all(&dtype.to_le_bytes()).unwrap();
            file.write_all(&0_u64.to_le_bytes()).unwrap();
        }
        drop(file);
        assert_eq!(dtype_label(&path), "F16");
    }

    #[test]
    fn tensor_element_counts_reject_overflow_and_accept_u64_boundary() {
        for (name, tensors, valid) in [
            ("dimension-product", vec![vec![u64::MAX, 2]], false),
            ("type-sum", vec![vec![u64::MAX], vec![1]], false),
            ("product-boundary", vec![vec![u64::MAX, 1]], true),
            ("sum-boundary", vec![vec![u64::MAX - 1], vec![1]], true),
        ] {
            let dir = crate::test_support::fixture_root("gguf-tests", name);
            let path = dir.join("counts.gguf");
            let mut file = File::create(&path).unwrap();
            file.write_all(b"GGUF").unwrap();
            file.write_all(&3_u32.to_le_bytes()).unwrap();
            file.write_all(&(tensors.len() as u64).to_le_bytes())
                .unwrap();
            file.write_all(&0_u64.to_le_bytes()).unwrap();
            for (index, dimensions) in tensors.iter().enumerate() {
                write_string(&mut file, &format!("weight.{index}"));
                file.write_all(&(dimensions.len() as u32).to_le_bytes())
                    .unwrap();
                for dimension in dimensions {
                    file.write_all(&dimension.to_le_bytes()).unwrap();
                }
                file.write_all(&1_u32.to_le_bytes()).unwrap();
                file.write_all(&0_u64.to_le_bytes()).unwrap();
            }
            drop(file);
            assert_eq!(detect_dtype(&path).is_ok(), valid, "{name}");
            assert_eq!(
                dtype_label(&path),
                if valid { "F16" } else { "unknown" },
                "{name}"
            );
        }
    }

    fn write_string(file: &mut File, s: &str) {
        file.write_all(&(s.len() as u64).to_le_bytes()).unwrap();
        file.write_all(s.as_bytes()).unwrap();
    }
}
