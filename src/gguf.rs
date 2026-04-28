//! Minimal GGUF metadata reader used for startup model dtype reporting.

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
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
pub fn detect_dtype(path: &Path) -> Result<Option<String>> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
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
                GGUF_TYPE_UINT32 => Ok(file_type_name(read_u32(&mut file)?).map(str::to_string)),
                GGUF_TYPE_INT32 => {
                    let value = read_i32(&mut file)?;
                    Ok(u32::try_from(value)
                        .ok()
                        .and_then(file_type_name)
                        .map(str::to_string))
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

fn dominant_tensor_type(file: &mut File, tensor_count: u64) -> Result<Option<&'static str>> {
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for _ in 0..tensor_count {
        let _name = read_string(file)?;
        let dims = read_u32(file)?;
        for _ in 0..dims {
            let _ = read_u64(file)?;
        }
        let tensor_type = read_u32(file)?;
        let _offset = read_u64(file)?;
        *counts.entry(tensor_type).or_default() += 1;
    }

    Ok(counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .and_then(|(tensor_type, _)| tensor_type_name(tensor_type)))
}

fn file_type_name(file_type: u32) -> Option<&'static str> {
    match file_type {
        0 => Some("F32"),
        1 => Some("F16"),
        2 => Some("Q4_0"),
        3 => Some("Q4_1"),
        7 => Some("Q8_0"),
        8 => Some("Q5_0"),
        9 => Some("Q5_1"),
        10 => Some("Q2_K"),
        11 => Some("Q3_K"),
        12 => Some("Q4_K"),
        13 => Some("Q5_K"),
        14 => Some("Q6_K"),
        24 => Some("BF16"),
        25 => Some("MXFP4"),
        26 => Some("NVFP4"),
        _ => None,
    }
}

fn tensor_type_name(tensor_type: u32) -> Option<&'static str> {
    match tensor_type {
        0 => Some("F32"),
        1 => Some("F16"),
        2 => Some("Q4_0"),
        3 => Some("Q4_1"),
        6 => Some("Q5_0"),
        7 => Some("Q5_1"),
        8 => Some("Q8_0"),
        10 => Some("Q2_K"),
        11 => Some("Q3_K"),
        12 => Some("Q4_K"),
        13 => Some("Q5_K"),
        14 => Some("Q6_K"),
        _ => None,
    }
}

fn skip_value(file: &mut File, value_type: u32) -> Result<()> {
    match value_type {
        GGUF_TYPE_UINT8 | GGUF_TYPE_INT8 | GGUF_TYPE_BOOL => skip_bytes(file, 1),
        GGUF_TYPE_UINT16 | GGUF_TYPE_INT16 => skip_bytes(file, 2),
        GGUF_TYPE_UINT32 | GGUF_TYPE_INT32 | GGUF_TYPE_FLOAT32 => skip_bytes(file, 4),
        GGUF_TYPE_UINT64 | GGUF_TYPE_INT64 | GGUF_TYPE_FLOAT64 => skip_bytes(file, 8),
        GGUF_TYPE_STRING => {
            let len = read_u64(file)?;
            skip_bytes(file, len)
        }
        GGUF_TYPE_ARRAY => {
            let item_type = read_u32(file)?;
            let len = read_u64(file)?;
            for _ in 0..len {
                skip_value(file, item_type)?;
            }
            Ok(())
        }
        other => Err(anyhow!("unknown GGUF metadata type {other}")),
    }
}

fn skip_bytes(file: &mut File, len: u64) -> Result<()> {
    let len = i64::try_from(len).context("GGUF value too large to seek past")?;
    file.seek(SeekFrom::Current(len))?;
    Ok(())
}

fn read_string(file: &mut File) -> Result<String> {
    let len = read_u64(file)?;
    if len > 1024 * 1024 {
        return Err(anyhow!("GGUF string length is too large: {len}"));
    }
    let mut buf = vec![0_u8; len as usize];
    file.read_exact(&mut buf)?;
    String::from_utf8(buf).context("GGUF string is not UTF-8")
}

fn read_u32(file: &mut File) -> Result<u32> {
    let mut buf = [0_u8; 4];
    file.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_i32(file: &mut File) -> Result<i32> {
    let mut buf = [0_u8; 4];
    file.read_exact(&mut buf)?;
    Ok(i32::from_le_bytes(buf))
}

fn read_u64(file: &mut File) -> Result<u64> {
    let mut buf = [0_u8; 8];
    file.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn detects_general_file_type() {
        let dir = Path::new("target/tmp/gguf-tests");
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("q8.gguf");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"GGUF").unwrap();
        file.write_all(&3_u32.to_le_bytes()).unwrap();
        file.write_all(&0_u64.to_le_bytes()).unwrap();
        file.write_all(&1_u64.to_le_bytes()).unwrap();
        write_string(&mut file, "general.file_type");
        file.write_all(&GGUF_TYPE_UINT32.to_le_bytes()).unwrap();
        file.write_all(&7_u32.to_le_bytes()).unwrap();
        drop(file);

        assert_eq!(detect_dtype(&path).unwrap().as_deref(), Some("Q8_0"));
    }

    fn write_string(file: &mut File, s: &str) {
        file.write_all(&(s.len() as u64).to_le_bytes()).unwrap();
        file.write_all(s.as_bytes()).unwrap();
    }
}
