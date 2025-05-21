use nusb::{DeviceInfo, Interface, transfer::RequestBuffer};
use probe_rs::probe::{DebugProbeError, ProbeCreationError};
use smol::future::FutureExt;
use smol::{Timer, block_on};
use std::io;
use std::time::Duration;
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
enum Command {
    Clock {
        tms: bool,
        tdi: bool,
        trst: bool,
        srst: bool,
    },
    Reset(bool),
}

impl From<Command> for u8 {
    fn from(value: Command) -> Self {
        match value {
            Command::Reset(x) => u8::from(Command::Clock {
                tms: true,
                tdi: true,
                trst: x,
                srst: false,
            }),
            Command::Clock {
                tms,
                tdi,
                trst,
                srst,
            } => {
                (u8::from(tms) << 1)
                    | (u8::from(tdi) << 4)
                    | (u8::from(trst) << 5)
                    | u8::from(srst) << 6
                    | 0
            }
        }
    }
}

impl Command {
    fn new(tms: bool, tdi: bool) -> Self {
        Command::Clock {
            tms,
            tdi,
            trst: false,
            srst: false,
        }
    }
}

struct ClockBuilder {
    buf: Vec<u8>,
}

impl ClockBuilder {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }
    fn add(mut self, command: Command) -> Self {
        let left = u8::from(command);
        let right = u8::from(left | 0x01);
        self.buf.push(left);
        self.buf.push(right);
        self
    }
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

        self.set_speed(60000).expect("Set speed error");
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

    fn send(&self, buf: &[u8]) {
        let mut out = vec![0xd2u8];
        let low = (buf.len() % 255) as u8;
        out.push(low);
        out.push(0);
        out.extend_from_slice(buf);
        let mut rev = [0; 64];
        self.device
            .write_bulk(self.epout, &out, Duration::from_millis(500))
            .expect("send error");

        self.device
            .read_bulk(self.epin, &mut rev, Duration::from_millis(500))
            .expect("read error");

        log::info!("rev: {:?}", rev);
    }
}

pub trait InterfaceExt {
    fn read_bulk(&self, endpoint: u8, buf: &mut [u8], timeout: Duration) -> io::Result<usize>;
    fn write_bulk(&self, endpoint: u8, buf: &[u8], timeout: Duration) -> io::Result<usize>;
}

impl InterfaceExt for Interface {
    fn write_bulk(&self, endpoint: u8, buf: &[u8], timeout: Duration) -> io::Result<usize> {
        let fut = async {
            let comp = self.bulk_out(endpoint, buf.to_vec()).await;
            comp.status.map_err(io::Error::other)?;

            let n = comp.data.actual_length();
            Ok(n)
        };

        block_on(fut.or(async {
            Timer::after(timeout).await;
            Err(std::io::ErrorKind::TimedOut.into())
        }))
    }

    fn read_bulk(&self, endpoint: u8, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        let fut = async {
            let comp = self.bulk_in(endpoint, RequestBuffer::new(buf.len())).await;
            comp.status.map_err(io::Error::other)?;

            let n = comp.data.len();
            buf[..n].copy_from_slice(&comp.data);
            Ok(n)
        };

        block_on(fut.or(async {
            Timer::after(timeout).await;
            Err(std::io::ErrorKind::TimedOut.into())
        }))
    }
}

fn main() {
    env_logger::init();
    let mut ch34x = CH34x::new_from_selector().expect("Not found ch34x device");
    ch34x.ch347_jtag_init();
    ch34x.send(&[0x10, 0x11, 0x10, 0x12, 0x13, 0x12]);
}
