//! Small checksum helpers shared by fetch and cache inspection.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Return the SHA256 digest for a file as lowercase hexadecimal.
///
/// # Arguments
///
/// * `path` - File to hash.
///
/// # Returns
///
/// A lowercase hexadecimal SHA256 digest.
///
/// # Errors
///
/// Returns an error if the file cannot be opened or read.
///
/// # Panics
///
/// Does not panic.
pub fn sha256_file_hex(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_digest(&hasher.finalize()))
}

/// Return bytes as lowercase hexadecimal.
///
/// # Arguments
///
/// * `bytes` - Digest bytes to render.
///
/// # Returns
///
/// Lowercase hexadecimal text.
///
/// # Panics
///
/// Does not panic.
pub fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_formatter_is_lowercase() {
        assert_eq!(hex_digest(&[0, 10, 255]), "000aff");
    }
}
