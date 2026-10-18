//! Host tests for firmware modules that don't depend on esp-hal.
//! Run: `cargo +stable test --target $(rustc +stable -vV | sed -n 's/host: //p')`
//! (the firmware's .cargo/config.toml would otherwise select the riscv target).
//! `old_structs.rs` is the pre-bounded packet definition, kept to check wire compatibility.
extern crate alloc;

#[path = "../../src/utils/battery.rs"]
pub mod battery;
#[path = "../../src/utils/bounded.rs"]
pub mod bounded;
#[path = "../../src/utils/ring_log.rs"]
pub mod ring_log;
#[path = "../../src/utils/text.rs"]
pub mod text;
#[path = "../../src/utils/lcd_text.rs"]
pub mod lcd_text;
#[path = "../../src/utils/oled_ui.rs"]
pub mod oled_ui;
#[path = "../../src/utils/stackmat.rs"]
pub mod stackmat;
pub mod utils {
    pub use crate::{bounded, text};
}
#[path = "../../src/structs.rs"]
pub mod structs;
#[path = "old_structs.rs"]
pub mod old_structs;
#[path = "../../src/bluetooth/codec.rs"]
pub mod ble_codec;

#[cfg(test)]
mod ble_tests {
    use crate::ble_codec::*;

    #[test]
    fn bond_round_trip() {
        for random in [false, true] {
            let buf = encode_bond([9, 8, 7, 6, 5, 4], [3; 16], 2, random);
            assert_eq!(
                decode_bond(&buf),
                Some(([9, 8, 7, 6, 5, 4], [3; 16], 2, random)),
            );
        }
    }

    #[test]
    fn static_identity_uses_ble_byte_order_and_static_bits() {
        assert_eq!(
            static_address_from_mac([0xb0, 0x81, 0x84, 0xfe, 0x7e, 0x9c]),
            [0x9c, 0x7e, 0xfe, 0x84, 0x81, 0xf0],
        );
        for first in [0x00, 0x40, 0x80, 0xc0] {
            let addr = static_address_from_mac([first, 1, 2, 3, 4, 5]);
            assert_eq!(addr[5] & 0xc0, 0xc0);
            assert_eq!(&addr[..5], &[5, 4, 3, 2, 1]);
        }
    }

    #[test]
    fn rejects_bad_bonds() {
        assert_eq!(decode_bond(&[]), None);
        assert_eq!(decode_bond(&[0; 32]), None); // old format: re-pair
        assert_eq!(decode_bond(&[0; 23]), None); // missing address type
        let mut buf = [0u8; BOND_LEN];
        buf[22] = 3; // unknown security level
        assert_eq!(decode_bond(&buf), None);
        buf[22] = 1;
        buf[23] = 2; // unknown address type
        assert_eq!(decode_bond(&buf), None);
    }

    #[test]
    fn parses_device_names() {
        // flags, then complete local name
        let adv = [
            2, 0x01, 0x06, 9, 0x09, b'F', b'K', b'M', b'D', b'-', b'1', b'2', b'3',
        ];
        assert_eq!(parse_device_name(&adv), Some("FKMD-123"));
        assert!(is_fkm_display("FKMD-123"));
        assert!(!is_fkm_display("Phone"));
        // shortened name ending exactly at the buffer end
        assert_eq!(parse_device_name(&[3, 0x08, b'a', b'b']), Some("ab"));
        // no name, zero length, truncated, empty, invalid utf8
        assert_eq!(parse_device_name(&[2, 0x01, 0x06]), None);
        assert_eq!(parse_device_name(&[0, 0x09, b'a']), None);
        assert_eq!(parse_device_name(&[5, 0x09, b'a']), None);
        assert_eq!(parse_device_name(&[]), None);
        assert_eq!(parse_device_name(&[2, 0x09, 0xff]), None);
        // never panics on arbitrary input
        for n in 0..=255u8 {
            let junk = [n, n.wrapping_mul(7), 0x09, n, 1, 0x08];
            let _ = parse_device_name(&junk);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{old_structs as old, structs as new};
    use serde_json::{from_str, to_string, Value};

    // every variant the firmware sends or receives, with realistic values
    const PACKETS: &[&str] = &[
        r#"{"tag":12,"data":{"solve":{"solve_time":12345,"penalty":-1,"competitor_id":7,"judge_id":9,"timestamp":1700000000,"session_id":"0b6a7d2e-1f4c-4f7e-9b1a-2c3d4e5f6a7b","delegate":false,"inspection_time":8000,"group_id":"333-r1-g2","sign_key":42}}}"#,
        r#"{"data":{"card_info_request":{"card_id":123,"is_competitor":true,"sign_key":5}}}"#,
        r#"{"data":{"card_info_request":{"card_id":123,"is_competitor":false,"attendance_device":true,"sign_key":5}}}"#,
        r#"{"data":{"battery":{"level":87.5,"voltage":3.91}}}"#,
        r#"{"data":{"battery":{"level":null,"voltage":null}}}"#,
        r#"{"data":{"add":{"firmware":"STATION","sign_key":77}}}"#,
        r#"{"tag":3,"data":{"card_info_response":{"card_id":1,"display":"Jan Kowalski","country_iso2":"PL","can_compete":true,"possible_groups":[{"groupId":"333-r1","name":"3x3 R1","secondaryText":null,"useInspection":true,"limit":600000},{"groupId":"222","name":"2x2","secondaryText":"Group B","useInspection":false,"limit":null}]}}}"#,
        r#"{"tag":4,"data":{"solve_confirm":{"competitor_id":1,"session_id":"0b6a7d2e-1f4c-4f7e-9b1a-2c3d4e5f6a7b","message":"Solve saved: 12.34"}}}"#,
        r#"{"tag":5,"data":{"api_error":{"error":"Competitor not found","should_reset_time":true}}}"#,
        r#"{"data":{"delegate_response":{"should_scan_cards":true,"solve_time":1000,"penalty":2}}}"#,
        r#"{"data":{"custom_message":{"line1":"Hello","line2":"World"}}}"#,
        r#"{"data":{"start_update":{"version":"D1700000000","build_time":0,"size":1234567,"crc":99,"firmware":"STATION"}}}"#,
        r#"{"data":{"device_settings":{"added":true,"locales":[{"locale":"pl","translations":[{"key":"A","translation":"Zażółć \"gęślą\""}]}],"default_locale":"en","fkm_token":1,"secure_rfid":false,"auto_setup":true,"sound_enabled":true}}}"#,
        r#"{"data":{"epoch_time":{"current_epoch":1700000000}}}"#,
        r#"{"data":{"set_device_settings":{"volume":5}}}"#,
        r#"{"data":"attendance_marked"}"#,
        r#"{"data":"dump_crash_log"}"#,
    ];

    #[test]
    fn same_json_as_before() {
        for json in PACKETS {
            let o: old::TimerPacket = from_str(json).unwrap_or_else(|e| panic!("old {json}: {e}"));
            let n: new::TimerPacket = from_str(json).unwrap_or_else(|e| panic!("new {json}: {e}"));
            let (os, ns) = (to_string(&o).unwrap(), to_string(&n).unwrap());
            assert_eq!(os, ns, "serialization differs for {json}");
            assert_eq!(from_str::<Value>(&ns).unwrap(), from_str::<Value>(json).unwrap());
        }
    }

    #[test]
    fn oversized_input_is_truncated_not_rejected() {
        let long = "x".repeat(500);
        let json = format!(
            r#"{{"data":{{"api_error":{{"error":"{long}","should_reset_time":false}}}}}}"#
        );
        let n: new::TimerPacket = from_str(&json).unwrap();
        let new::TimerPacketInner::ApiError(e) = n.data else { panic!() };
        assert_eq!(e.error.len(), 128);

        let groups = (0..20)
            .map(|i| format!(r#"{{"groupId":"g{i}","name":"n","secondaryText":null,"useInspection":true,"limit":null}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!(
            r#"{{"data":{{"card_info_response":{{"card_id":1,"display":"d","country_iso2":"pl","can_compete":true,"possible_groups":[{groups}]}}}}}}"#
        );
        let n: new::TimerPacket = from_str(&json).unwrap();
        let new::TimerPacketInner::CardInfoResponse(r) = n.data else { panic!() };
        assert_eq!(r.possible_groups.len(), new::MAX_POSSIBLE_GROUPS);
    }
}

/// Outgoing packets are serialized with serde-json-core on the device; it must produce the
/// same bytes serde_json did.
#[cfg(test)]
mod outgoing_json {
    use crate::structs::{TimerPacket, TimerPacketInner};

    fn same(p: &TimerPacket) {
        let mut buf = [0u8; 1024];
        let n = serde_json_core::to_slice(p, &mut buf).expect("serde-json-core");
        assert_eq!(core::str::from_utf8(&buf[..n]).unwrap(), serde_json::to_string(p).unwrap());
    }

    #[test]
    fn identical_to_serde_json() {
        let packets = [
            TimerPacketInner::Solve {
                solve_time: 12345,
                penalty: -1,
                competitor_id: 7,
                judge_id: 9,
                timestamp: 1_700_000_000,
                session_id: "0b6a7d2e-1f4c-4f7e-9b1a-2c3d4e5f6a7b".into(),
                delegate: false,
                inspection_time: 8000,
                group_id: "333-r1-g2 \"q\" \\ ż".into(),
                sign_key: u32::MAX,
            },
            TimerPacketInner::CardInfoRequest { card_id: u64::MAX, is_competitor: true, attendance_device: None, sign_key: 5 },
            TimerPacketInner::CardInfoRequest { card_id: 1, is_competitor: false, attendance_device: Some(true), sign_key: 5 },
            TimerPacketInner::Battery { level: Some(87.5), voltage: Some(3.91) },
            TimerPacketInner::Battery { level: Some(0.1), voltage: Some(4.123456789) },
            TimerPacketInner::Battery { level: Some(100.0), voltage: Some(1e-7) },
            TimerPacketInner::Battery { level: None, voltage: None },
            TimerPacketInner::Add { firmware: "STATION".into(), sign_key: 77 },
        ];
        for data in packets {
            same(&TimerPacket { tag: None, data: data.clone() });
            same(&TimerPacket { tag: Some(u64::MAX), data });
        }
    }
}
