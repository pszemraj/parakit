//! Input device selection, change-detection identity, and microphone metadata.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};

#[cfg(target_os = "linux")]
use super::super::pactl::{pactl_default_source_info, pactl_default_source_name};
use super::TARGET_RATE;

/// Summary of the active microphone stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MicInfo {
    /// CPAL device name.
    pub name: String,
    /// Input stream sample rate opened from the OS/audio server.
    pub input_rate: u32,
    /// Number of input channels opened.
    pub channels: u16,
    /// CPAL sample format label.
    pub sample_format: String,
    /// PulseAudio/PipeWire source name when available.
    pub source_id: Option<String>,
    /// Whether the stream is resampled to the Parakeet target rate.
    pub resampling: bool,
    /// Human-readable note about why the opened input config was selected.
    pub config_note: Option<String>,
}

impl MicInfo {
    /// Return the concise startup/device-change label.
    ///
    /// # Returns
    ///
    /// A human-readable device summary.
    pub fn summary(&self) -> String {
        let channel_label = input_channel_label(self.channels);
        let rate_label = if self.resampling {
            format!(
                "{} Hz {} input -> {} Hz mono model",
                self.input_rate, channel_label, TARGET_RATE
            )
        } else if self.channels == 1 {
            format!("{} Hz input/model", self.input_rate)
        } else {
            format!(
                "{} Hz {} input -> mono model",
                self.input_rate, channel_label
            )
        };
        format!("{}, {}, {}", self.name, rate_label, self.sample_format)
    }

    /// Return detailed audio routing notes for verbose diagnostics.
    ///
    /// # Returns
    ///
    /// Lines describing the capture and model input shape.
    pub fn detail_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(format!("model input: {} Hz mono PCM", TARGET_RATE));
        if self.channels > 1 {
            lines.push(format!(
                "capture path: CPAL opened {}; callback downmixes to mono before resampling",
                input_channel_label(self.channels)
            ));
        } else if self.resampling {
            lines.push("capture path: mono input, resampling to model rate".to_string());
        } else {
            lines.push("capture path: mono input, no resampling".to_string());
        }
        if let Some(note) = &self.config_note {
            lines.push(format!("input config: {note}"));
        }
        lines
    }

    /// Return whether this input appears to be a Bluetooth microphone.
    ///
    /// # Returns
    ///
    /// `true` when the source name or device label contains common Bluetooth
    /// identifiers.
    pub fn looks_bluetooth(&self) -> bool {
        is_bluetooth_input_name(&self.name)
            || self
                .source_id
                .as_deref()
                .is_some_and(is_bluetooth_input_name)
    }
}

fn input_channel_label(channels: u16) -> String {
    if channels == 1 {
        "mono".to_string()
    } else {
        format!("{channels}ch")
    }
}

/// Input device chosen by [`select_input_device`] with its preferred stream config.
pub(super) struct SelectedInput {
    pub(super) device: cpal::Device,
    name: String,
    pub(super) config: cpal::SupportedStreamConfig,
    config_note: Option<String>,
    is_default: bool,
}

/// Comparable identity of an opened input, used to detect device changes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MicIdentity {
    pub(super) name: String,
    pub(super) source_id: Option<String>,
    pub(super) input_rate: u32,
    pub(super) channels: u16,
    pub(super) sample_format: String,
}

/// Pick the input device, preferring the OS default and physical inputs over virtual ones.
///
/// # Returns
///
/// The selected device with its preferred input config.
///
/// # Errors
///
/// Returns an error if the input devices cannot be listed or none is usable.
pub(super) fn select_input_device(host: &cpal::Host) -> Result<SelectedInput> {
    let default_device = host.default_input_device();
    let default_name = default_device.as_ref().and_then(|d| d.name().ok());
    if let Some(device) = default_device {
        let name = default_name
            .clone()
            .unwrap_or_else(|| "<default input>".to_string());
        if !is_virtual_input_name(&name) {
            if let Ok(config) = device.default_input_config() {
                let (config, config_note) = select_preferred_input_config(&device, config);
                return Ok(SelectedInput {
                    device,
                    name,
                    config,
                    config_note,
                    is_default: true,
                });
            }
        }
    }

    let mut physical = Vec::new();
    let mut virtual_inputs = Vec::new();

    for device in host
        .input_devices()
        .context("failed to list input devices")?
    {
        let name = device
            .name()
            .unwrap_or_else(|_| "<unknown input>".to_string());
        let (config, config_note) = match device.default_input_config() {
            Ok(config) => select_preferred_input_config(&device, config),
            Err(_) => continue,
        };
        let selected = SelectedInput {
            device,
            name: name.clone(),
            config,
            config_note,
            is_default: default_name.as_deref() == Some(name.as_str()),
        };
        if is_virtual_input_name(&name) {
            virtual_inputs.push(selected);
        } else {
            physical.push(selected);
        }
    }

    if let Some(default_name) = default_name.as_deref() {
        if !is_virtual_input_name(default_name) {
            if let Some(index) = physical.iter().position(|d| d.name == default_name) {
                return Ok(physical.swap_remove(index));
            }
        }
    }

    if let Some(device) = physical.into_iter().next() {
        return Ok(device);
    }

    if let Some(default_name) = default_name {
        if let Some(index) = virtual_inputs.iter().position(|d| d.name == default_name) {
            return Ok(virtual_inputs.swap_remove(index));
        }
    }

    virtual_inputs
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no usable input device"))
}

fn select_preferred_input_config(
    device: &cpal::Device,
    default_config: cpal::SupportedStreamConfig,
) -> (cpal::SupportedStreamConfig, Option<String>) {
    if default_config.channels() == 1 {
        return (default_config, None);
    }

    let default_channels = default_config.channels();
    let ranges = match device.supported_input_configs() {
        Ok(ranges) => ranges,
        Err(err) => {
            return (
                default_config,
                Some(format!(
                    "could not inspect alternate input configs ({err}); downmixing {default_channels}ch input to mono"
                )),
            );
        }
    };
    let ranges = ranges.collect::<Vec<_>>();

    match preferred_mono_config_from_ranges(&default_config, ranges.iter()) {
        Some(config) => (
            config,
            Some(format!(
                "selected same-rate/same-format mono input instead of {default_channels}ch default"
            )),
        ),
        None => {
            let mut note = format!(
                "no same-rate/same-format mono input config advertised; downmixing {default_channels}ch input to mono"
            );
            if let Some(alternate) = lower_cost_mono_config_note(&default_config, &ranges) {
                note.push_str("; ");
                note.push_str(&alternate);
            }
            (default_config, Some(note))
        }
    }
}

/// Find a mono config at the default sample rate and format.
///
/// # Returns
///
/// The matching mono config, or `None` when no advertised range offers one.
///
/// # Arguments
///
/// * `default_config` - OS default input config whose rate and format must match.
/// * `ranges` - Advertised input config ranges to search.
pub(super) fn preferred_mono_config_from_ranges<'a, I>(
    default_config: &cpal::SupportedStreamConfig,
    ranges: I,
) -> Option<cpal::SupportedStreamConfig>
where
    I: IntoIterator<Item = &'a cpal::SupportedStreamConfigRange>,
{
    let default_rate = default_config.sample_rate();
    let default_format = default_config.sample_format();

    ranges.into_iter().find_map(|range| {
        if range.channels() == 1 && range.sample_format() == default_format {
            range.try_with_sample_rate(default_rate)
        } else {
            None
        }
    })
}

/// Explain why an advertised mono config was not selected.
///
/// # Returns
///
/// A note about a same-rate or target-rate mono alternative, if one exists.
///
/// # Arguments
///
/// * `default_config` - OS default input config that was kept.
/// * `ranges` - Advertised input config ranges to search for alternatives.
pub(super) fn lower_cost_mono_config_note(
    default_config: &cpal::SupportedStreamConfig,
    ranges: &[cpal::SupportedStreamConfigRange],
) -> Option<String> {
    let default_rate = default_config.sample_rate();
    let default_format = default_config.sample_format();
    let same_rate_other_format = ranges.iter().find_map(|range| {
        if range.channels() == 1 && range.sample_format() != default_format {
            range.try_with_sample_rate(default_rate)
        } else {
            None
        }
    });
    if let Some(config) = same_rate_other_format {
        return Some(format!(
            "same-rate mono is available as {:?}, but not selected because it changes sample format",
            config.sample_format()
        ));
    }

    let target_rate = cpal::SampleRate(TARGET_RATE);
    let target_rate_mono = ranges.iter().find_map(|range| {
        if range.channels() == 1 {
            range.try_with_sample_rate(target_rate)
        } else {
            None
        }
    });
    target_rate_mono.map(|config| {
        format!(
            "{} Hz mono is available as {:?}, but not selected because the current policy preserves the OS default sample rate",
            TARGET_RATE,
            config.sample_format()
        )
    })
}

/// Describe the input device that would currently be selected.
///
/// # Returns
///
/// The microphone summary for the selected input.
///
/// # Errors
///
/// Returns an error if no usable input device is available.
pub(super) fn selected_mic_info(host: &cpal::Host) -> Result<MicInfo> {
    let selected = select_input_device(host)?;
    Ok(mic_snapshot_from_selected(&selected).0)
}

/// Poll the identity of the input device that would currently be selected.
///
/// # Returns
///
/// The source-aware identity for the selected input.
///
/// # Errors
///
/// Returns an error if no usable input device is available.
pub(super) fn selected_mic_identity(host: &cpal::Host) -> Result<MicIdentity> {
    let selected = select_input_device(host)?;
    Ok(polled_mic_identity_from_selected(&selected))
}

/// Build the display info and change-detection identity for a selected input.
///
/// # Returns
///
/// The microphone summary paired with its source-aware identity.
pub(super) fn mic_snapshot_from_selected(selected: &SelectedInput) -> (MicInfo, MicIdentity) {
    let raw_identity = raw_mic_identity_from_selected(selected);
    let mut info = mic_info_from_identity(&raw_identity);
    enhance_mic_info(&mut info, selected.is_default);
    info.config_note = selected.config_note.clone();
    if info.source_id.is_none() {
        info.source_id = default_source_id_for_identity(selected);
    }
    let identity = source_aware_mic_identity(raw_identity, info.source_id.clone());
    (info, identity)
}

fn raw_mic_identity_from_selected(selected: &SelectedInput) -> MicIdentity {
    mic_identity_from_config(&selected.name, &selected.config)
}

fn polled_mic_identity_from_selected(selected: &SelectedInput) -> MicIdentity {
    let raw_identity = raw_mic_identity_from_selected(selected);
    source_aware_mic_identity(raw_identity, default_source_id_for_identity(selected))
}

/// Attach a resolved source id to an identity.
///
/// # Returns
///
/// The identity carrying `source_id`.
///
/// # Arguments
///
/// * `identity` - Raw identity built from the stream config.
/// * `source_id` - Resolved PulseAudio/PipeWire source name, if any.
pub(super) fn source_aware_mic_identity(
    mut identity: MicIdentity,
    source_id: Option<String>,
) -> MicIdentity {
    identity.source_id = source_id;
    identity
}

/// Return whether an input is the OS-selected default source, eligible for a
/// pactl default-source lookup.
#[cfg(target_os = "linux")]
fn is_default_source_candidate(is_default: bool, name: &str) -> bool {
    is_default || name == "default"
}

#[cfg(target_os = "linux")]
fn default_source_id_for_identity(selected: &SelectedInput) -> Option<String> {
    if !is_default_source_candidate(selected.is_default, &selected.name) {
        return None;
    }
    pactl_default_source_name()
}

#[cfg(not(target_os = "linux"))]
fn default_source_id_for_identity(_selected: &SelectedInput) -> Option<String> {
    None
}

fn mic_identity_from_config(name: &str, config: &cpal::SupportedStreamConfig) -> MicIdentity {
    MicIdentity {
        name: name.to_string(),
        source_id: None,
        input_rate: config.sample_rate().0,
        channels: config.channels(),
        sample_format: format!("{:?}", config.sample_format()),
    }
}

/// Build display info from an identity, without a config note.
///
/// # Returns
///
/// The microphone summary for `identity`.
pub(super) fn mic_info_from_identity(identity: &MicIdentity) -> MicInfo {
    MicInfo {
        name: identity.name.clone(),
        input_rate: identity.input_rate,
        channels: identity.channels,
        sample_format: identity.sample_format.clone(),
        source_id: identity.source_id.clone(),
        resampling: identity.input_rate != TARGET_RATE,
        config_note: None,
    }
}

#[cfg(target_os = "linux")]
fn enhance_mic_info(info: &mut MicInfo, is_default: bool) {
    if !is_default_source_candidate(is_default, &info.name) {
        return;
    }
    let Some(source) = pactl_default_source_info() else {
        return;
    };
    let source_id = source.name.clone();
    info.name = source.description.unwrap_or(source.name);
    if let Some(rate) = source.rate {
        info.input_rate = rate;
    }
    if let Some(channels) = source.channels {
        info.channels = channels;
    }
    if let Some(format) = source.sample_format {
        info.sample_format = format;
    }
    info.source_id = Some(source_id);
    info.resampling = info.input_rate != TARGET_RATE;
}

#[cfg(not(target_os = "linux"))]
fn enhance_mic_info(_info: &mut MicInfo, _is_default: bool) {}

/// Return whether `name`, lowercased, contains any of `patterns`.
fn contains_any_pattern(name: &str, patterns: &[&str]) -> bool {
    let lower = name.to_lowercase();
    patterns.iter().any(|pattern| lower.contains(pattern))
}

/// Return whether a device name looks like a monitor or virtual input.
///
/// # Returns
///
/// `true` for names parakit should avoid unless no physical-looking input is
/// available.
pub(super) fn is_virtual_input_name(name: &str) -> bool {
    contains_any_pattern(
        name,
        &[
            "monitor of",
            ".monitor",
            " monitor",
            "loopback",
            "virtual",
            "null",
            "dummy",
            "blackhole",
            "soundflower",
            "stereo mix",
            "what u hear",
            "wasapi output",
        ],
    )
}

/// Return whether an input name or source id looks like a Bluetooth microphone.
///
/// # Returns
///
/// `true` for common Bluetooth transport, profile, and headset labels.
pub(super) fn is_bluetooth_input_name(name: &str) -> bool {
    contains_any_pattern(
        name,
        &[
            "bluetooth",
            "bluez",
            "headset_head_unit",
            "headset-head-unit",
            "handsfree",
            "hands-free",
            "hands free",
            "hfp",
            "hsp",
            "a2dp",
            "airpod",
            "earbud",
            "earbuds",
            "galaxy buds",
            "pixel buds",
            "freebuds",
        ],
    )
}
