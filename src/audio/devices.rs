//! Finding audio devices and turning the user's choice into a concrete device.

use anyhow::{Context, Result, bail};
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub hardware: Hardware,
}

/// What the system reports about the physical device behind an endpoint.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hardware {
    /// Shared by all endpoints of one physical device (Windows' container id).
    pub container: Option<String>,
    /// A Bluetooth hands-free endpoint: using it puts the device in call mode.
    pub hands_free: bool,
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

/// Connected devices of one direction, without device handles: cheap to share.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceList {
    /// In system order.
    pub devices: Vec<DeviceInfo>,
    pub default: Option<DeviceInfo>,
}

/// Connected microphones and outputs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceLists {
    pub inputs: DeviceList,
    pub outputs: DeviceList,
}

impl DeviceLists {
    pub fn get(&self, direction: Direction) -> &DeviceList {
        match direction {
            Direction::Input => &self.inputs,
            Direction::Output => &self.outputs,
        }
    }
}

/// The devices connected at one moment, with the handles needed to open them.
pub struct Inventory {
    inputs: Scan,
    outputs: Scan,
}

struct Scan {
    devices: Vec<(Device, DeviceInfo)>,
    default_id: Option<String>,
}

impl Scan {
    fn new(host: &Host, direction: Direction) -> Result<Self> {
        let devices: Box<dyn Iterator<Item = Device>> = match direction {
            Direction::Input => Box::new(host.input_devices()?),
            Direction::Output => Box::new(host.output_devices()?),
        };
        let devices = devices.filter_map(|device| describe(&device).map(|info| (device, info))).collect();
        Ok(Self { devices, default_id: platform::default_device_id(host, direction) })
    }

    fn find(&self, id: &str) -> Option<&(Device, DeviceInfo)> {
        self.devices.iter().find(|(_, info)| info.id == id)
    }

    fn default(&self) -> Option<&(Device, DeviceInfo)> {
        self.find(self.default_id.as_deref()?)
    }
}

impl Inventory {
    pub fn scan(host: &Host) -> Result<Self> {
        Ok(Self { inputs: Scan::new(host, Direction::Input)?, outputs: Scan::new(host, Direction::Output)? })
    }

    pub fn lists(&self) -> DeviceLists {
        let list = |scan: &Scan| DeviceList {
            devices: scan.devices.iter().map(|(_, info)| info.clone()).collect(),
            default: scan.default().map(|(_, info)| info.clone()),
        };
        DeviceLists { inputs: list(&self.inputs), outputs: list(&self.outputs) }
    }

    /// First device whose name contains `query` (case-insensitive).
    pub fn find_by_name(&self, direction: Direction, query: &str) -> Option<&DeviceInfo> {
        let query = query.to_lowercase();
        self.get(direction).devices.iter().map(|(_, info)| info).find(|info| info.name.to_lowercase().contains(&query))
    }

    /// Picks the device to open: the chosen one if connected, otherwise the system default.
    /// Returns `None` when the choice is [`DeviceChoice::Off`].
    pub fn resolve(&self, direction: Direction, choice: &DeviceChoice) -> Result<Option<Resolved>> {
        let scan = self.get(direction);
        let wanted = match choice {
            DeviceChoice::Off => return Ok(None),
            DeviceChoice::Default => None,
            DeviceChoice::Device { id, .. } => Some(id.as_str()),
        };
        if let Some((device, info)) = wanted.and_then(|id| scan.find(id)) {
            return Ok(Some(Resolved { device: device.clone(), info: info.clone(), is_fallback: false }));
        }
        let kind = match direction {
            Direction::Input => "microphone",
            Direction::Output => "output device",
        };
        if scan.default_id.is_none() {
            bail!("There is no default {kind}.");
        }
        let (device, info) = scan.default().with_context(|| format!("The default {kind} is not available."))?;
        Ok(Some(Resolved { device: device.clone(), info: info.clone(), is_fallback: wanted.is_some() }))
    }

    fn get(&self, direction: Direction) -> &Scan {
        match direction {
            Direction::Input => &self.inputs,
            Direction::Output => &self.outputs,
        }
    }
}

/// Whether `output` is on the same Bluetooth device as a hands-free microphone.
/// A Bluetooth headset whose microphone is in use is in call mode: its stereo
/// output stops working and only low-quality call audio is available. Other
/// devices (USB headsets, built-in audio) can use both sides at once.
pub fn shares_hands_free_device(input: &DeviceInfo, output: &DeviceInfo) -> bool {
    let (input, output) = (&input.hardware, &output.hardware);
    input.hands_free && input.container.is_some() && input.container == output.container
}

fn describe(device: &Device) -> Option<DeviceInfo> {
    let id = device.id().ok()?;
    let name = device.description().map(|d| d.name().to_owned()).unwrap_or_else(|_| "Unknown device".into());
    Some(DeviceInfo { hardware: platform::hardware(&id), id: id.to_string(), name })
}

#[cfg(windows)]
mod platform {
    //! cpal's own "default device" follows the system default through
    //! `ActivateAudioInterfaceAsync`, which fails on threads where COM is already
    //! initialized in apartment mode (`RPC_E_CHANGED_MODE`). Instead we ask Windows
    //! which endpoint is the default and open that one directly; the engine notices
    //! when the default changes and reconnects.
    //!
    //! Hardware details come from standard endpoint properties, available for any
    //! device: the bus enumerator and the container id.

    use super::{Direction, Hardware};
    use cpal::Host;
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eConsole, eRender};
    use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, STGM_READ,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::core::{GUID, HSTRING};

    /// `DEVPKEY_Device_EnumeratorName`: the bus the device is attached through.
    const ENUMERATOR_NAME: PROPERTYKEY =
        PROPERTYKEY { fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 24 };
    /// `DEVPKEY_Device_ContainerId`: the physical device an endpoint belongs to.
    const CONTAINER_ID: PROPERTYKEY =
        PROPERTYKEY { fmtid: GUID::from_u128(0x8c7ed206_3f8a_4827_b3ab_ae9e1faefc6c), pid: 2 };
    /// Enumerator of classic Bluetooth hands-free (HFP/HSP) audio endpoints.
    const HANDS_FREE_ENUMERATOR: &str = "BTHHFENUM";

    thread_local! {
        static ENUMERATOR: Option<IMMDeviceEnumerator> = unsafe {
            // Same mode cpal uses; harmless if COM is already initialized on this thread.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()
        };
    }

    pub fn hardware(id: &cpal::DeviceId) -> Hardware {
        ENUMERATOR
            .with(|e| unsafe {
                let store = e.as_ref()?.GetDevice(&HSTRING::from(id.id())).ok()?.OpenPropertyStore(STGM_READ).ok()?;
                let enumerator = read_string(&store, &ENUMERATOR_NAME);
                Some(Hardware {
                    container: read_string(&store, &CONTAINER_ID),
                    hands_free: enumerator.is_some_and(|name| name.eq_ignore_ascii_case(HANDS_FREE_ENUMERATOR)),
                })
            })
            .unwrap_or_default()
    }

    unsafe fn read_string(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<String> {
        unsafe {
            let mut value = store.GetValue(key).ok()?;
            let text = PropVariantToStringAlloc(&value).ok().and_then(|text| {
                let owned = text.to_string().ok();
                CoTaskMemFree(Some(text.0 as *const _));
                owned
            });
            let _ = PropVariantClear(&mut value);
            text
        }
    }

    pub fn default_device_id(host: &Host, direction: Direction) -> Option<String> {
        let flow = match direction {
            Direction::Input => eCapture,
            Direction::Output => eRender,
        };
        let endpoint = ENUMERATOR.with(|e| unsafe {
            let id = e.as_ref()?.GetDefaultAudioEndpoint(flow, eConsole).ok()?.GetId().ok()?;
            let text = id.to_string().ok();
            CoTaskMemFree(Some(id.0 as *const _)); // allocated by COM, owned by the caller
            text
        })?;
        Some(cpal::DeviceId::new(host.id(), endpoint).to_string())
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{Direction, Hardware};
    use cpal::Host;
    use cpal::traits::{DeviceTrait, HostTrait};

    pub fn hardware(_id: &cpal::DeviceId) -> Hardware {
        Hardware::default()
    }

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

    fn endpoint(container: Option<&str>, hands_free: bool) -> DeviceInfo {
        DeviceInfo { hardware: Hardware { container: container.map(Into::into), hands_free }, ..Default::default() }
    }

    #[test]
    fn only_a_hands_free_microphone_blocks_its_own_device() {
        let headset_mic = endpoint(Some("headset"), true);
        assert!(shares_hands_free_device(&headset_mic, &endpoint(Some("headset"), false)));
        assert!(!shares_hands_free_device(&headset_mic, &endpoint(Some("speakers"), false)));
        // A USB headset or built-in audio can record and play at once.
        assert!(!shares_hands_free_device(&endpoint(Some("usb"), false), &endpoint(Some("usb"), false)));
        // Without a container id nothing can be concluded.
        assert!(!shares_hands_free_device(&endpoint(None, true), &endpoint(None, false)));
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
