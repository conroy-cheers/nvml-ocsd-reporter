use log::{debug, info, warn};
use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use nvml_wrapper::error::NvmlError;
use nvml_wrapper::struct_wrappers::device::PciInfo;
use ocsd::OcsdHeader;
use ocsd::{
    client::OcsdContext, Celsius, MemoryMapped, OcsdDevice, OcsdDeviceHeader, OcsdSensor,
    OcsdSensorLocation, OcsdSensorStatus, OcsdSensorType,
};
use serde::{Deserialize, Serialize};
use simple_logger::SimpleLogger;
use std::fmt::Debug;
use std::process::exit;
use std::sync::{atomic, Arc};
use std::time::Duration;
use std::{cmp::min, fs::OpenOptions};
use systemd_journal_logger::{connected_to_journal, JournalLog};

use nvml_wrapper::Nvml;

fn format_struct_bytes(bytes: &Vec<u8>) -> String {
    let num_chunks = bytes.len() / 8;

    let mut output = String::new();
    for chunk_idx in 0..num_chunks {
        let max_idx = min((chunk_idx + 1) * 8, bytes.len());
        for b in &bytes[chunk_idx * 8..max_idx] {
            output.push_str(format!("{b:02x} ").as_str());
        }
        if chunk_idx % 2 == 0 {
            output.push(' ');
        } else {
            output.push('\n');
        }
    }
    output
}

fn format_ocsd_report(device: OcsdDevice) -> String {
    format!(
        "Device header:\n{}\nDevice Sensor 0:\n{}\n",
        format_struct_bytes(&device.header.to_bytes()),
        format_struct_bytes(&device.sensors[0].to_bytes())
    )
}

#[derive(Serialize, Deserialize, Debug)]
struct AppState {
    count: u16,
    original_buffers_in_use: u8,
}

trait HasSlot {
    fn slot(&self) -> Option<u8>;
}

impl HasSlot for PciInfo {
    fn slot(&self) -> Option<u8> {
        match self.bus {
            0x04 => Some(1),
            0x0a => Some(2),
            _default => None,
        }
    }
}

#[derive(Debug, Clone)]
struct DeviceInfo {
    slot: Option<u8>,
    pci_bus: u8,
    current_temperature_celsius: i16,
    temperature_threshold_celsius: Option<i16>,
    name: String,
    count: u16,
}

fn format_device_info(index: usize, device: &DeviceInfo) -> String {
    format!(
        "{}\n{}\n{}\n",
        format!(
            "Device {:?} with index {} in {}",
            device.name,
            index,
            match device.slot {
                Some(slot) => format!("slot #{}:", slot),
                None => format!("unknown slot:"),
            }
        ),
        format!(
            "Current temperature: {}C",
            device.current_temperature_celsius
        ),
        match device.temperature_threshold_celsius {
            Some(threshold) => format!("Temperature threshold: {:?}C", threshold),
            None => format!("Temperature threshold: unknown"),
        }
    )
}

trait ReadDeviceInfo {
    fn read_device_info(&self, count: u16) -> Result<DeviceInfo, NvmlError>;
}

impl ReadDeviceInfo for nvml_wrapper::Device<'_> {
    fn read_device_info(&self, count: u16) -> Result<DeviceInfo, NvmlError> {
        let pci_info = self.pci_info()?;

        let slot = pci_info.slot();
        let pci_bus: u8 = pci_info.bus.try_into().unwrap();
        let current_temperature_celsius: i16 = self
            .temperature(TemperatureSensor::Gpu)?
            .try_into()
            .unwrap();
        let temperature_threshold_celsius: Option<i16> = match self.temperature_threshold(
            nvml_wrapper::enum_wrappers::device::TemperatureThreshold::Slowdown,
        ) {
            Ok(threshold) => Some((threshold - 20).try_into().unwrap()),
            Err(_) => Some(70),
        };
        let device_name = self.name()?;

        Ok(DeviceInfo {
            slot,
            pci_bus,
            current_temperature_celsius: current_temperature_celsius,
            // the below offset is required for correct temperatures
            // to be displayed in iLO, but also prevents fans from spinning
            // up until close to 90deg (which is generally not desirable)
            // + (temperature_threshold_celsius.unwrap() - 90),
            temperature_threshold_celsius,
            name: device_name,
            count,
        })
    }
}

impl From<DeviceInfo> for OcsdDevice {
    fn from(device: DeviceInfo) -> Self {
        let header = OcsdDeviceHeader {
            version: ocsd::DeviceVersion::Version1,
            pci_bus: device.pci_bus,
            pci_device: 0x00,
            flags_caps: 0x00000010,
        };

        let sensor = OcsdSensor {
            sensor_type: OcsdSensorType::Thermal,
            sensor_location: OcsdSensorLocation::InternalToAsic,
            configuration: 0x0000,
            status: OcsdSensorStatus::WithChecksum
                | OcsdSensorStatus::Present
                | OcsdSensorStatus::NotFailed,
            max_continuous_threshold: Celsius::new(
                device.temperature_threshold_celsius.unwrap_or(90),
            )
            .unwrap(),
            caution_threshold: Celsius::new(100).unwrap(),
            reading: Celsius::new(device.current_temperature_celsius).unwrap(),
            update_count: device.count,
            bus: Some(device.pci_bus),
        };

        OcsdDevice {
            header,
            sensors: [sensor, Default::default(), Default::default()],
        }
    }
}

fn print_nvml_devices(nvml_devices: &Vec<nvml_wrapper::Device>) -> Result<(), NvmlError> {
    info!("Detected {} devices.\n", nvml_devices.len());
    for (index, nvml_device) in (&nvml_devices).into_iter().enumerate() {
        let device_info = nvml_device.read_device_info(1)?;
        info!("\n{}", format_device_info(index, &device_info));

        let example_ocsd = OcsdDevice::from(device_info);
        debug!("\n{}", format_ocsd_report(example_ocsd));
    }

    let nvml_devices_without_slot: Vec<_> = nvml_devices
        .into_iter()
        .filter(|device| {
            device
                .pci_info()
                .map_or(false, |info| info.slot().is_none())
        })
        .collect();
    if nvml_devices_without_slot.len() > 0 {
        let plural = if nvml_devices_without_slot.len() == 1 {
            ("", "its address")
        } else {
            ("s", "their addresses")
        };
        warn!(
            "Found {} device{} without known PCI slot for {}:\n",
            nvml_devices_without_slot.len(),
            plural.0,
            plural.1
        );
        for device in nvml_devices_without_slot {
            let device_info = device.read_device_info(0)?;
            warn!("{} on bus {:02x}", device_info.name, device_info.pci_bus);
        }
    }
    Ok(())
}

const STATE_FILE_NAME: &str = "nvml_ocsd_reporter.json";

fn load_app_state(header: &OcsdHeader) -> AppState {
    match OpenOptions::new().read(true).open(STATE_FILE_NAME) {
        Ok(reader) => match serde_json::from_reader(reader) {
            Ok(app_state) => app_state,
            Err(err) => {
                warn!("Couldn't load state: {:?}", err);
                warn!("Using default.");
                AppState {
                    count: 0,
                    original_buffers_in_use: header.buffers_in_use,
                }
            }
        },
        Err(err) => {
            warn!("Couldn't open state file: {:?}", err);
            warn!("Using default.");
            AppState {
                count: 0,
                original_buffers_in_use: header.buffers_in_use,
            }
        }
    }
}

fn open_state_file() -> std::fs::File {
    OpenOptions::new()
        .create(true)
        .truncate(true) // If the file already exists we want to overwrite the old data
        .write(true)
        .open(STATE_FILE_NAME)
        .unwrap()
}

fn make_modified_header(
    original_header: &OcsdHeader,
    app_state: &AppState,
    devices: &Vec<nvml_wrapper::Device>,
) -> OcsdHeader {
    OcsdHeader {
        buffers_in_use: app_state.original_buffers_in_use + devices.len() as u8,
        ..*original_header
    }
}

fn main() -> Result<(), NvmlError> {
    if connected_to_journal() {
        JournalLog::new()
            .unwrap()
            .with_extra_fields(vec![("VERSION", env!("CARGO_PKG_VERSION"))])
            .install()
            .unwrap();
    } else {
        SimpleLogger::new().init().unwrap();
    }

    let nvml = Nvml::init()?;
    let num_devices = nvml.device_count()?;
    let nvml_devices: Vec<nvml_wrapper::Device> = (0..num_devices)
        .filter_map(|device_idx| match nvml.device_by_index(device_idx) {
            Ok(device) => Some(device),
            Err(_) => None,
        })
        .collect();

    if nvml_devices.len() > 0 {
        print_nvml_devices(&nvml_devices)?;
        let nvml_devices: Vec<_> = nvml_devices
            .into_iter()
            .filter(|device| {
                device
                    .pci_info()
                    .map_or(false, |info| info.slot().is_some())
            })
            .collect();

        match OcsdContext::new(ocsd::client::base_address::ML350_GEN9) {
            Ok(mut context) => {
                let original_header = context.read_header();
                debug!(
                    "Header data before write:\n{}",
                    format_struct_bytes(&original_header.to_bytes())
                );

                let app_state = load_app_state(&original_header);
                let count = Arc::new(atomic::AtomicU16::new(app_state.count));

                let header = make_modified_header(&original_header, &app_state, &nvml_devices);
                debug!(
                    "Writing header data:\n{}",
                    format_struct_bytes(&header.to_bytes())
                );
                context.write_header(&header);

                let should_exit = Arc::new(atomic::AtomicBool::new(false));
                let mut file = open_state_file();

                let should_exit_clone = should_exit.clone();
                let count_clone = count.clone();
                let _ = ctrlc::set_handler(move || {
                    serde_json::to_writer(
                        &mut file,
                        &AppState {
                            count: (*count_clone).load(atomic::Ordering::Relaxed),
                            ..app_state
                        },
                    )
                    .unwrap();
                    should_exit_clone.store(true, atomic::Ordering::Relaxed);
                });

                /// Gets (0-based) OCSD slot index
                fn ocsd_slot_for_index(index: usize, app_state: &AppState) -> usize {
                    (index + app_state.original_buffers_in_use as usize).into()
                }

                // Zero out option cards beyond the originals, in case we left something behind on previous run
                for index in
                    0..(original_header.max_option_cards - app_state.original_buffers_in_use)
                {
                    context.device_mappings[ocsd_slot_for_index(index as usize, &app_state)].write(
                        &OcsdDevice {
                            header: OcsdDeviceHeader {
                                version: ocsd::DeviceVersion::Unknown,
                                pci_bus: 0,
                                pci_device: 0,
                                flags_caps: 0,
                            },
                            sensors: Default::default(),
                        },
                    )
                }

                loop {
                    for (index, device) in nvml_devices.iter().enumerate() {
                        let device_info =
                            device.read_device_info((*count).load(atomic::Ordering::Relaxed))?;
                        context.device_mappings[ocsd_slot_for_index(index, &app_state)]
                            .write(&device_info.into());
                    }

                    std::thread::sleep(Duration::from_millis(1000));
                    (*count).fetch_add(1, atomic::Ordering::Relaxed);

                    if should_exit.load(atomic::Ordering::Relaxed) {
                        break;
                    };
                }
            }
            Err(_) => {
                info!(
                    "Unable to open OCSD header context in memory. Do you have access to /dev/mem?"
                )
            }
        }
    } else {
        warn!("No devices detected.");
        exit(0);
    }

    Ok(())
}
