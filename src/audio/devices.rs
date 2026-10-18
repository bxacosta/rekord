//! Finding audio devices and turning the user's choice into a concrete device.

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, Host};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
}

/// Which device the user wants. Persisted in the config file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "use", rename_all = "lowercase")]
pub enum DeviceChoice {
    /// Follow the system default device, even when it changes.
    #[default]
    Default,
    /// A specific device. The name is kept to describe it while it is unplugged.
    Device { id: String, name: String },
    /// No device (only meaningful for the output: don't monitor).
    Off,
}

impl DeviceChoice {
    pub fn device(info: &DeviceInfo) -> Self {
        Self::Device { id: info.id.clone(), name: info.name.clone() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
}

impl fmt::Display for DeviceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// The device that will actually be opened for a [`DeviceChoice`].
pub struct Resolved {
    pub device: Device,
    pub info: DeviceInfo,
    /// The chosen device is not connected, so the system default is used instead.
    pub is_fallback: bool,
}

/// Connected devices, in system order.
pub fn list(host: &Host, direction: Direction) -> Result<Vec<DeviceInfo>> {
    Ok(devices(host, direction)?.filter_map(|d| describe(&d)).collect())
}

/// First device whose name contains `query` (case-insensitive).
pub fn find_by_name(host: &Host, direction: Direction, query: &str) -> Result<Option<DeviceInfo>> {
    let query = query.to_lowercase();
    Ok(list(host, direction)?.into_iter().find(|d| d.name.to_lowercase().contains(&query)))
}

/// Picks the device to open: the chosen one if connected, otherwise the system default.
/// Returns `None` when the choice is [`DeviceChoice::Off`].
pub fn resolve(host: &Host, direction: Direction, choice: &DeviceChoice) -> Result<Option<Resolved>> {
    let wanted = match choice {
        DeviceChoice::Off => return Ok(None),
        DeviceChoice::Default => None,
        DeviceChoice::Device { id, .. } => Some(id.as_str()),
    };
    if let Some(wanted) = wanted
        && let Some((device, info)) = find(host, direction, |info| info.id == wanted)?
    {
        return Ok(Some(Resolved { device, info, is_fallback: false }));
    }
    let (device, info) = default_device(host, direction)?;
    Ok(Some(Resolved { device, info, is_fallback: wanted.is_some() }))
}

/// The system default device, as a concrete device.
pub fn default_device(host: &Host, direction: Direction) -> Result<(Device, DeviceInfo)> {
    let kind = match direction {
        Direction::Input => "microphone",
        Direction::Output => "output device",
    };
    let default =
        platform::default_device_id(host, direction).with_context(|| format!("There is no default {kind}."))?;
    find(host, direction, |info| info.id == default)?.with_context(|| format!("The default {kind} is not available."))
}

/// Whether two endpoints belong to the same physical device. Bluetooth headsets expose
/// e.g. "Headset (Earphones)" and "Headphones (Earphones)": monitoring through the same
/// headset whose microphone is in use forces it into low-quality call mode.
pub fn same_hardware(a: &str, b: &str) -> bool {
    fn inner(name: &str) -> Option<String> {
        let (_, rest) = name.split_once('(')?;
        let (inner, _) = rest.rsplit_once(')')?;
        Some(inner.trim().to_lowercase())
    }
    matches!((inner(a), inner(b)), (Some(x), Some(y)) if x == y)
}

fn devices(host: &Host, direction: Direction) -> Result<Box<dyn Iterator<Item = Device>>> {
    Ok(match direction {
        Direction::Input => Box::new(host.input_devices()?),
        Direction::Output => Box::new(host.output_devices()?),
    })
}

fn describe(device: &Device) -> Option<DeviceInfo> {
    let id = device.id().ok()?.to_string();
    let name = device.description().map(|d| d.name().to_owned()).unwrap_or_else(|_| "Unknown device".into());
    Some(DeviceInfo { id, name })
}

fn find(
    host: &Host,
    direction: Direction,
    matches: impl Fn(&DeviceInfo) -> bool,
) -> Result<Option<(Device, DeviceInfo)>> {
    Ok(devices(host, direction)?.find_map(|device| {
        let info = describe(&device).filter(&matches)?;
        Some((device, info))
    }))
}

#[cfg(windows)]
mod platform {
    //! cpal's own "default device" follows the system default through
    //! `ActivateAudioInterfaceAsync`, which fails on threads where COM is already
    //! initialized in apartment mode (`RPC_E_CHANGED_MODE`). Instead we ask Windows
    //! which endpoint is the default and open that one directly; the engine notices
    //! when the default changes and reconnects.

    use super::Direction;
    use cpal::Host;
    use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eConsole, eRender};
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};

    pub fn default_device_id(host: &Host, direction: Direction) -> Option<String> {
        thread_local! {
            static ENUMERATOR: Option<IMMDeviceEnumerator> = unsafe {
                // Same mode cpal uses; harmless if COM is already initialized on this thread.
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()
            };
        }
        let flow = match direction {
            Direction::Input => eCapture,
            Direction::Output => eRender,
        };
        let endpoint = ENUMERATOR.with(|e| unsafe {
            e.as_ref()?.GetDefaultAudioEndpoint(flow, eConsole).ok()?.GetId().ok()?.to_string().ok()
        })?;
        Some(cpal::DeviceId::new(host.id(), endpoint).to_string())
    }
}

#[cfg(not(windows))]
mod platform {
    use super::Direction;
    use cpal::Host;
    use cpal::traits::{DeviceTrait, HostTrait};

    pub fn default_device_id(host: &Host, direction: Direction) -> Option<String> {
        let device = match direction {
            Direction::Input => host.default_input_device(),
            Direction::Output => host.default_output_device(),
        }?;
        Some(device.id().ok()?.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_endpoints_of_the_same_headset() {
        assert!(same_hardware("Headset (Earphones)", "Headphones (earphones)"));
        assert!(!same_hardware("Microphone (Realtek(R) Audio)", "Speakers (Qudelix-5K)"));
        assert!(!same_hardware("Microphone", "Speakers"));
    }

    #[test]
    fn device_choice_round_trips_through_toml() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Wrapper {
            input: DeviceChoice,
            output: DeviceChoice,
        }
        let value = Wrapper {
            input: DeviceChoice::Device { id: "wasapi:x".into(), name: "Mic".into() },
            output: DeviceChoice::Off,
        };
        let text = toml::to_string(&value).unwrap();
        assert_eq!(toml::from_str::<Wrapper>(&text).unwrap(), value);
    }
}
