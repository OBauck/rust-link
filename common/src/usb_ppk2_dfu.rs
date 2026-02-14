use core::marker::PhantomData;

use embassy_usb::control::{InResponse, Recipient, Request, RequestType};
use embassy_usb::driver::Driver;
use embassy_usb::{Builder, Handler};
use static_cell::StaticCell;

struct DfuHandler;

impl Handler for DfuHandler {
    fn control_in<'a>(&'a mut self, req: Request, buf: &'a mut [u8]) -> Option<InResponse<'a>> {
        #[cfg(feature = "defmt")]
        defmt::trace!("Request: {:?}", defmt::Debug2Format(&req));
        if (req.request_type, req.recipient, req.index)
            != (RequestType::Class, Recipient::Device, 0)
        {
            return None;
        }
        const DFU_VERSION_STR_REPLY: &str = "power_profiler_kit_2 1.2.4-db16a94";
        let reply = DFU_VERSION_STR_REPLY.as_bytes();
        buf[0..reply.len()].copy_from_slice(reply);
        Some(InResponse::Accepted(&buf[0..reply.len()]))
    }
}

pub struct Ppk2DfuClass<'d, D: Driver<'d>> {
    _marker: &'d PhantomData<D>,
}

impl<'d, D: Driver<'d>> Ppk2DfuClass<'d, D> {
    pub fn new(builder: &mut Builder<'d, D>) {
        const USB_CLASS_APPN_SPEC: u8 = 0xFF;
        const APPN_SPEC_SUBCLASS_DFU: u8 = 0x01;
        const DFU_PROTOCOL_DFU: u8 = 0x01;
        const DESC_DFU_FUNCTIONAL: u8 = 0x21;

        let mut func = builder.function(0x00, 0x00, 0x00);

        let mut iface = func.interface();
        let comm_if = u8::from(iface.interface_number());
        let mut alt = iface.alt_setting(
            USB_CLASS_APPN_SPEC,
            APPN_SPEC_SUBCLASS_DFU,
            DFU_PROTOCOL_DFU,
            None,
        );
        alt.descriptor(
            DESC_DFU_FUNCTIONAL,
            &[
                9, comm_if, comm_if, 0, 0, 0x10, 0x01, // DFU 1.1
            ],
        );

        drop(func);

        static DFU_HANDLER: StaticCell<DfuHandler> = StaticCell::new();
        let dfu_handler = DFU_HANDLER.init(DfuHandler);
        builder.handler(dfu_handler);
    }
}
