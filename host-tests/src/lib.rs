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
#[path = "../../src/utils/live_header.rs"]
pub mod live_header;
#[path = "../../src/utils/live_send.rs"]
pub mod live_send;

#[cfg(test)]
mod live_send_tests {
    use crate::live_send::*;

    #[test]
    fn active_sends_every_second() {
        assert!(!should_send(true, false, 999, false));
        assert!(should_send(true, false, 1000, false));
    }

    #[test]
    fn idle_sends_keep_alive_every_5_s() {
        for ms in [1000, 2000, 4999] {
            assert!(!should_send(false, false, ms, false));
        }
        assert!(should_send(false, false, 5000, false));
        // a never-sent station sends at once
        assert!(should_send(false, false, u64::MAX, false));
    }

    #[test]
    fn idle_logs_still_go_every_second() {
        assert!(!should_send(false, true, 999, false));
        assert!(should_send(false, true, 1000, false));
    }

    #[test]
    fn change_sends_at_once_but_rate_limited() {
        for (active, has_logs) in [(false, false), (true, false), (false, true), (true, true)] {
            assert!(!should_send(active, has_logs, 0, true));
            assert!(!should_send(active, has_logs, 99, true));
            assert!(should_send(active, has_logs, 100, true));
            assert!(should_send(active, has_logs, 400, true));
        }
    }

    #[test]
    fn flapping_change_is_capped_at_10_per_second() {
        // a change on every 10 ms tick for 1 s
        let (mut last, mut sent) = (None::<u64>, 0);
        for now in (0..1000).step_by(10) {
            let since = last.map_or(u64::MAX, |l| now - l);
            if should_send(false, false, since, true) {
                last = Some(now);
                sent += 1;
            }
        }
        assert_eq!(sent, 10);
    }
}

#[cfg(test)]
mod live_header_tests {
    use crate::live_header::*;

    fn header(timer_running: bool, inspection_running: bool) -> LogsHeader<'static> {
        LogsHeader {
            current_time: Some(12_345),
            inspection_time: None,
            competitor: Some(7),
            group_id: Some("333-r1-g2"),
            session_id: Some("0b6a7d2e-1f4c-4f7e-9b1a-2c3d4e5f6a7b"),
            timer_running,
            inspection_running,
        }
    }

    /// A batch with no log lines, written over stale bytes like the reused buffer half.
    fn header_only(header: &LogsHeader) -> [u8; HEADER_LEN] {
        let mut batch = [0xAA; HEADER_LEN];
        init_batch(&mut batch);
        write_header(&mut batch, header);
        batch
    }

    #[test]
    fn header_only_batch_layout() {
        let batch = header_only(&header(true, false));
        assert_eq!(batch.len(), 100);
        assert_eq!(&batch[..2], b"L\x00");
        assert_eq!(batch[2..10], 12_345u64.to_be_bytes());
        assert_eq!(batch[10..18], u64::MAX.to_be_bytes());
        assert_eq!(batch[18..26], 7u64.to_be_bytes());
        assert_eq!(&batch[26..35], b"333-r1-g2");
        assert!(batch[35..46].iter().all(|&b| b == 0));
        assert_eq!(&batch[46..82], b"0b6a7d2e-1f4c-4f7e-9b1a-2c3d4e5f6a7b");
        assert_eq!(&batch[82..84], b"LV");
        assert_eq!(batch[84], 1);
        assert_eq!(batch[85], 0b01);
        assert!(batch[86..].iter().all(|&b| b == 0));
    }

    #[test]
    fn flags_follow_running_state() {
        for (timer, inspection, flags) in [
            (false, false, 0b00),
            (true, false, 0b01),
            (false, true, 0b10),
            (true, true, 0b11),
        ] {
            assert_eq!(header_only(&header(timer, inspection))[85], flags);
        }
    }

    #[test]
    fn empty_state_is_none_and_zeroes() {
        let batch = header_only(&LogsHeader {
            current_time: None,
            inspection_time: None,
            competitor: None,
            group_id: None,
            session_id: None,
            timer_running: false,
            inspection_running: false,
        });
        for field in [2..10, 10..18, 18..26] {
            assert_eq!(batch[field], u64::MAX.to_be_bytes());
        }
        assert!(batch[26..82].iter().all(|&b| b == 0));
        assert_eq!(&batch[82..86], b"LV\x01\x00");
    }

    #[test]
    fn header_keeps_truncated_flag_and_log_lines() {
        let mut batch = [0u8; HEADER_LEN + 4];
        init_batch(&mut batch);
        batch[1] = 0x01; // set by the logger when the half filled up
        batch[HEADER_LEN..].copy_from_slice(&[0, 2, b'I', b' ']);
        write_header(&mut batch, &header(false, true));
        assert_eq!(&batch[..2], b"L\x01");
        assert_eq!(batch[85], 0b10);
        assert_eq!(batch[HEADER_LEN..], [0, 2, b'I', b' ']);
    }

    #[test]
    fn oversized_ids_are_left_empty() {
        let long = "x".repeat(37);
        let batch = header_only(&LogsHeader {
            group_id: Some(&long[..21]),
            session_id: Some(&long),
            ..header(false, false)
        });
        assert!(batch[26..82].iter().all(|&b| b == 0));
        assert_eq!(&batch[82..84], b"LV");
    }
}

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
