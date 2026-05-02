//! PulseAudio/PipeWire source enrichment through `pactl`.

/// Human-readable source details parsed from `pactl list sources`.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct PactlSourceInfo {
    /// PulseAudio/PipeWire source name.
    pub(crate) name: String,
    /// Human-readable source description.
    pub(crate) description: Option<String>,
    /// Source sample rate.
    pub(crate) rate: Option<u32>,
    /// Source channel count.
    pub(crate) channels: Option<u16>,
    /// Source sample format label.
    pub(crate) sample_format: Option<String>,
}

/// Read the current default PulseAudio/PipeWire source with `pactl`.
///
/// # Returns
///
/// Parsed details for the default source, or `None` when `pactl` is missing or
/// the output cannot be matched.
pub(crate) fn pactl_default_source_info() -> Option<PactlSourceInfo> {
    let default = std::process::Command::new("pactl")
        .args(["get-default-source"])
        .output()
        .ok()?;
    if !default.status.success() {
        return None;
    }
    let default_name = String::from_utf8_lossy(&default.stdout).trim().to_string();
    if default_name.is_empty() {
        return None;
    }

    let sources = std::process::Command::new("pactl")
        .args(["list", "sources"])
        .output()
        .ok()?;
    if !sources.status.success() {
        return None;
    }
    let sources = String::from_utf8_lossy(&sources.stdout);
    parse_pactl_sources(&sources)
        .into_iter()
        .find(|source| source.name == default_name)
}

fn parse_pactl_sources(text: &str) -> Vec<PactlSourceInfo> {
    let mut out = Vec::new();
    let mut current: Option<PactlSourceInfo> = None;

    for line in text.lines() {
        if line.starts_with("Source #") {
            if let Some(source) = current.take() {
                out.push(source);
            }
            current = Some(PactlSourceInfo::default());
            continue;
        }

        let Some(source) = current.as_mut() else {
            continue;
        };
        let trimmed = line.trim_start();
        if let Some(name) = trimmed.strip_prefix("Name: ") {
            source.name = name.trim().to_string();
        } else if let Some(description) = trimmed.strip_prefix("Description: ") {
            source.description = Some(description.trim().to_string());
        } else if let Some(spec) = trimmed.strip_prefix("Sample Specification: ") {
            let (sample_format, channels, rate) = parse_sample_spec(spec.trim());
            source.sample_format = sample_format;
            source.channels = channels;
            source.rate = rate;
        }
    }

    if let Some(source) = current {
        out.push(source);
    }
    out
}

fn parse_sample_spec(spec: &str) -> (Option<String>, Option<u16>, Option<u32>) {
    let mut parts = spec.split_whitespace();
    let sample_format = parts.next().map(str::to_string);
    let channels = parts
        .next()
        .and_then(|part| part.strip_suffix("ch"))
        .and_then(|part| part.parse().ok());
    let rate = parts
        .next()
        .and_then(|part| part.strip_suffix("Hz"))
        .and_then(|part| part.parse().ok());
    (sample_format, channels, rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pactl_source_parser_extracts_description_and_rate() {
        let sources = parse_pactl_sources(
            r#"Source #42
    Name: alsa_input.usb-Test_Speech_Mic-00.mono-fallback
    Description: USB Speech Mic Mono
    Sample Specification: s24le 1ch 48000Hz
Source #43
    Name: alsa_output.pci-0000_00.monitor
    Description: Monitor of HDMI Audio
    Sample Specification: s32le 2ch 48000Hz
"#,
        );
        assert_eq!(sources.len(), 2);
        assert_eq!(
            sources[0],
            PactlSourceInfo {
                name: "alsa_input.usb-Test_Speech_Mic-00.mono-fallback".to_string(),
                description: Some("USB Speech Mic Mono".to_string()),
                rate: Some(48_000),
                channels: Some(1),
                sample_format: Some("s24le".to_string()),
            }
        );
    }
}
