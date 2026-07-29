//! C ABI and MeshCore C++ adapters for Project Mycelium's virtual radio bus.

mod ffi;

pub use ffi::{
    meshemu_bus_tick, meshemu_radio_create, meshemu_radio_destroy, meshemu_radio_get_est_airtime,
    meshemu_radio_get_rssi, meshemu_radio_get_snr, meshemu_radio_is_send_complete,
    meshemu_radio_recv_raw, meshemu_radio_set_position, meshemu_radio_start_send,
};

#[cfg(test)]
mod tests {
    use std::ffi::{c_void, CString};
    use std::sync::Mutex;

    use super::*;

    static TEST_BUS: Mutex<()> = Mutex::new(());

    fn create(id: &str, position: (f64, f64)) -> *mut c_void {
        let id = CString::new(id).unwrap();
        unsafe { meshemu_radio_create(id.as_ptr(), 915.0, 125, 7, 5, 14.0, position.0, position.1) }
    }

    fn receive(radio: *mut c_void) -> Vec<u8> {
        let mut buffer = [0_u8; 255];
        let len =
            unsafe { meshemu_radio_recv_raw(radio, buffer.as_mut_ptr(), buffer.len() as i32) };
        buffer[..len.max(0) as usize].to_vec()
    }

    fn send(radio: *mut c_void, packet: &[u8]) -> bool {
        unsafe { meshemu_radio_start_send(radio, packet.as_ptr(), packet.len() as u32) }
    }

    fn destroy(radio: *mut c_void) {
        unsafe { meshemu_radio_destroy(radio) }
    }

    #[test]
    fn two_radios_exchange_a_packet_and_report_link_metrics() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let sender = create("sender", (51.5074, -0.1278));
        let receiver = create("receiver", (51.5075, -0.1278));
        assert!(!sender.is_null());
        assert!(!receiver.is_null());

        let packet = [1, 2, 3, 4];
        assert!(send(sender, &packet));
        assert_eq!(receive(receiver), packet);
        assert!(unsafe { meshemu_radio_get_rssi(receiver) } < 0.0);
        assert!(unsafe { meshemu_radio_get_snr(receiver) } > 0.0);
        assert_eq!(unsafe { meshemu_radio_get_est_airtime(sender, 16) }, 56);
        assert!(meshemu_radio_is_send_complete(sender));

        destroy(receiver);
        destroy(sender);
    }

    #[test]
    fn moving_a_radio_out_of_range_prevents_reception() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let sender = create("sender", (0.0, 0.0));
        let receiver = create("receiver", (0.0, 0.0001));

        unsafe { meshemu_radio_set_position(receiver, 0.0, 180.0) };
        meshemu_bus_tick(1_000);
        let packet = [42];
        assert!(send(sender, &packet));
        assert!(receive(receiver).is_empty());

        destroy(receiver);
        destroy(sender);
    }

    #[test]
    fn position_updates_can_bring_a_radio_into_range() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let sender = create("sender", (0.0, 0.0));
        let receiver = create("receiver", (0.0, 180.0));
        let packet = [7, 8, 9];

        assert!(send(sender, &packet));
        assert!(receive(receiver).is_empty());

        unsafe { meshemu_radio_set_position(receiver, 0.0, 0.0001) };
        meshemu_bus_tick(1_000);
        assert!(send(sender, &packet));
        assert_eq!(receive(receiver), packet);

        destroy(receiver);
        destroy(sender);
    }

    #[test]
    fn destroy_unregisters_the_node_and_releases_its_identifier() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();
        let first = create("reusable", (0.0, 0.0));
        assert!(!first.is_null());
        assert!(create("reusable", (0.0, 0.0)).is_null());

        destroy(first);
        let replacement = create("reusable", (0.0, 0.0));
        assert!(!replacement.is_null());
        destroy(replacement);
    }

    #[test]
    fn null_and_invalid_ffi_arguments_fail_safely() {
        let _serial = TEST_BUS.lock().unwrap();
        ffi::reset_bus();

        assert!(unsafe {
            meshemu_radio_create(std::ptr::null(), 915.0, 125, 7, 5, 14.0, 0.0, 0.0)
        }
        .is_null());
        assert!(!unsafe { meshemu_radio_start_send(std::ptr::null_mut(), std::ptr::null(), 1) });
        assert_eq!(
            unsafe { meshemu_radio_recv_raw(std::ptr::null_mut(), std::ptr::null_mut(), 0) },
            0
        );
        destroy(std::ptr::null_mut());
    }
}
