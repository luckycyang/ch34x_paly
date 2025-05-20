use nusb::{DeviceInfo, Interface, transfer::RequestBuffer};
use probe_rs::probe::{DebugProbeError, ProbeCreationError};

const CH34X_VID_PID: [(u16, u16); 3] = [(0x1A86, 0x55DE), (0x1A86, 0x55DD), (0x1A86, 0x55E8)];

pub(crate) fn is_ch34x_device(device: &DeviceInfo) -> bool {
    CH34X_VID_PID.contains(&(device.vendor_id(), device.product_id()))
}
#[derive(Debug)]
enum PACK {
    STANDARD_PACK,
    LARGER_PACK,
}

struct CH34x {
    device: Interface,
    name: String,
    epout: u8,
    epin: u8,
    pack: Option<PACK>,
}

impl CH34x {
    fn new_from_selector() -> Result<Self, ProbeCreationError> {
        let device = nusb::list_devices()
            .map_err(ProbeCreationError::Usb)?
            .filter(is_ch34x_device)
            .next()
            .ok_or(ProbeCreationError::NotFound)?;

        // tracing::debug!("{:?}", device);

        let device_handle = device
            .open()
            .map_err(probe_rs::probe::ProbeCreationError::Usb)?;

        let config = device_handle
            .configurations()
            .next()
            .expect("Can get usb device configs");

        log::info!("Active config descriptor: {:?}", config);

        for interface in config.interfaces() {
            let interface_number = interface.interface_number();

            let Some(descriptor) = interface.alt_settings().next() else {
                continue;
            };

            if (!(descriptor.class() != 255
                && descriptor.subclass() != 0
                && descriptor.protocol() != 0))
            {
                continue;
            }
        }

        // if should use config to get current interface, this only work in ch347f
        let interface = device_handle
            .claim_interface(4)
            .map_err(ProbeCreationError::Usb)?;

        Ok(Self {
            device: interface,
            name: "ch347".into(),
            epout: 0x06,
            epin: 0x86,
            pack: None,
        })
    }

    fn ch347_jtag_init(&mut self) {
        smol::block_on(
            self.device
                .bulk_out(6, vec![0xD0, 6, 0, 0, 9u8, 0x00, 0x00, 0x00, 0x00]),
        )
        .into_result()
        .expect("Can init Jtag Device");

        let rev = smol::block_on(self.device.bulk_in(self.epin, RequestBuffer::new(4)))
            .into_result()
            .unwrap();

        if let Some(val) = rev.last() {
            log::info!("Last value is {}", *val);
            self.pack = if *val == 0x00 {
                Some(PACK::STANDARD_PACK)
            } else {
                Some(PACK::LARGER_PACK)
            }
        }

        self.set_speed(15000).expect("Set speed error");
    }

    fn send(&self, data: Vec<u8>) -> bool {
        match smol::block_on(self.device.bulk_out(self.epout, data)).into_result() {
            Err(_) => return false,
            _ => {}
        }

        return true;
    }

    fn set_speed(&mut self, speed_khz: u32) -> Result<u32, DebugProbeError> {
        let index = self.speed_khz_index(speed_khz)?;
        log::info!("Get speed index: {}", index);
        let buf = vec![0xD0, 0x06, 0x00, 0x00, index, 0x00, 0x00, 0x00, 0x00];
        smol::block_on(self.device.bulk_out(self.epout, buf))
            .into_result()
            .unwrap();

        let rev = smol::block_on(self.device.bulk_in(self.epin, RequestBuffer::new(4)))
            .into_result()
            .unwrap();
        if *(rev.last().unwrap()) != 0x00 {
            return Err(DebugProbeError::UnsupportedSpeed(speed_khz));
        }
        Ok(speed_khz)
    }

    fn speed_khz_index(&self, speed: u32) -> Result<u8, DebugProbeError> {
        let index;
        log::info!(
            "pack mode: {:?}, seek speed index for {}khz",
            self.pack,
            speed
        );
        match self.pack {
            Some(PACK::STANDARD_PACK) => {
                index = match speed {
                    1875 => 0,
                    3750 => 1,
                    7500 => 2,
                    15000 => 3,
                    30000 => 4,
                    60000 => 5,
                    _ => return Err(DebugProbeError::UnsupportedSpeed(speed)),
                };
            }
            Some(PACK::LARGER_PACK) => {
                index = match speed {
                    468 => 0,
                    937 => 1,
                    1875 => 2,
                    3750 => 3,
                    7500 => 4,
                    15000 => 5,
                    30000 => 6,
                    60000 => 7,
                    _ => return Err(DebugProbeError::UnsupportedSpeed(speed)),
                }
            }
            None => {
                return Err(DebugProbeError::UnsupportedSpeed(speed));
            }
        }
        Ok(index)
    }
}

enum Command {
    Clock {
        tms: bool,
        tdi: bool,
        trst: bool,
        srst: bool,
    },
}

impl From<Command> for Vec<u8> {
    fn from(value: Command) -> Self {
        match value {
            Command::Clock {
                tms,
                tdi,
                trst,
                srst,
            } => {
                let low = (u8::from(tms) << 1
                    | u8::from(tdi) << 4
                    | u8::from(trst) << 5
                    | u8::from(srst) << 6
                    | 0u8);
                let hight = low | 1u8;
                vec![low, hight]
            }
        }
    }
}

fn main() {
    env_logger::init();
    let mut ch34x = CH34x::new_from_selector().expect("Not found ch34x device");
    ch34x.ch347_jtag_init();
}
