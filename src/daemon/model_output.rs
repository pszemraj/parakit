//! Filter only the pinned Parakeet loader's unconditional informational lines.
//!
//! Quiet model reloads use this filter. Unlike the discard guard used for
//! quiet startup, it forwards every other stderr line, including concurrent
//! microphone, sound, and IPC errors.

use std::io::{self, BufWriter, Read, Write};

const FILTERED_PREFIXES: [&[u8]; 2] = [
    b"parakeet: vocab=",
    b"parakeet: BN folded into conv_dw weights for ",
];

/// Run model initialization while preserving all stderr except known loader info.
///
/// # Returns
///
/// The closure result, also when redirection is unavailable.
pub(crate) fn with_model_output_filtered<T>(f: impl FnOnce() -> T) -> T {
    super::stderr::with_stderr_filtered(f, |reader, writer| {
        let _ = filter_lines(reader, writer);
    })
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum LineDisposition {
    Undecided,
    Forward,
    Suppress,
}

fn filter_lines(mut reader: impl Read, writer: impl Write) -> io::Result<()> {
    let mut writer = BufWriter::new(writer);
    let mut buffer = [0_u8; 8192];
    let mut prefix = Vec::with_capacity(
        FILTERED_PREFIXES
            .iter()
            .map(|candidate| candidate.len())
            .max()
            .unwrap_or_default(),
    );
    let mut disposition = LineDisposition::Undecided;

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            if disposition == LineDisposition::Undecided {
                writer.write_all(&prefix)?;
            }
            return writer.flush();
        }

        for &byte in &buffer[..read] {
            match disposition {
                LineDisposition::Undecided => {
                    prefix.push(byte);
                    if FILTERED_PREFIXES
                        .iter()
                        .any(|candidate| prefix.starts_with(candidate))
                    {
                        prefix.clear();
                        disposition = LineDisposition::Suppress;
                    } else if !FILTERED_PREFIXES
                        .iter()
                        .any(|candidate| candidate.starts_with(&prefix))
                    {
                        writer.write_all(&prefix)?;
                        prefix.clear();
                        disposition = LineDisposition::Forward;
                    }
                }
                LineDisposition::Forward => writer.write_all(&[byte])?,
                LineDisposition::Suppress => {}
            }

            if byte == b'\n' {
                prefix.clear();
                disposition = LineDisposition::Undecided;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_preserves_errors_and_concurrent_diagnostics() {
        let input = b"parakeet: vocab=8192\nparakit: warning: microphone disconnected\nparakeet: BN folded into conv_dw weights for 24 layers\nparakeet: failed to allocate backend buffer\npartial";
        let mut output = Vec::new();
        filter_lines(&input[..], &mut output).unwrap();
        assert_eq!(output, b"parakit: warning: microphone disconnected\nparakeet: failed to allocate backend buffer\npartial");
    }

    #[test]
    fn long_unknown_lines_are_forwarded_verbatim() {
        let mut input = vec![b'x'; 8192];
        input.extend_from_slice(b"parakeet: vocab=important diagnostic tail\n");
        input.extend_from_slice(&vec![b'y'; 100_000]);
        let mut output = Vec::new();
        filter_lines(&input[..], &mut output).unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn filtered_prefix_split_across_reads_is_still_suppressed() {
        struct ShortReads<'a> {
            bytes: &'a [u8],
        }

        impl Read for ShortReads<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                let len = self.bytes.len().min(buffer.len()).min(3);
                buffer[..len].copy_from_slice(&self.bytes[..len]);
                self.bytes = &self.bytes[len..];
                Ok(len)
            }
        }

        let mut output = Vec::new();
        filter_lines(
            ShortReads {
                bytes: b"parakeet: vocab=8192\nkept\n",
            },
            &mut output,
        )
        .unwrap();
        assert_eq!(output, b"kept\n");
    }
}
