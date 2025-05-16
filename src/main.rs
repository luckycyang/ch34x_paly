use log::info;
use nusb::Interface;

const CH34x_IDVENDOR: u16 = 0x1A86;
const Ch34x_IDPRODUCT: u16 = 0x55DE;

struct CH34x {
    device: Interface,
    name: String,
}

fn main() {
    env_logger::init();
    let mut devices =
        nusb::list_devices().expect("Can't found usb devices, please check you permission");
    let handle = devices
        .find(|f| f.vendor_id() == CH34x_IDVENDOR && f.product_id() == Ch34x_IDPRODUCT)
        .expect("Not found CH34x device");

    let device = handle.open().expect("Can't open Ch34x device");
    // jtag interface number is 4
    let jtag_interface = device
        .claim_interface(4)
        .expect("Open jtag interface error");

    let x = smol::block_on(jtag_interface.bulk_out(6, "hello".as_bytes().into())).into_result();

    match x {
        Ok(x) => info!("{:?}", x),
        Err(_) => info!("trans err!!!"),
    }

    info!("Hello, world!");
}
