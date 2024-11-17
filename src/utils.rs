pub fn ms_to_time_str(ms: u64) -> heapless::String<8> {
    let minutes: u8 = (ms / 60000) as u8;
    let seconds: u8 = ((ms % 60000) / 1000) as u8;
    let ms: u16 = (ms % 1000) as u16;

    let mut time_str = heapless::String::<8>::new();
    if minutes > 0 {
        _ = time_str.push((minutes + b'0') as char);
        _ = time_str.push(':');
        _ = time_str.push_str(&alloc::format!("{seconds:02}.{ms:03}"));
    } else {
        _ = time_str.push_str(&alloc::format!("{seconds:01}.{ms:03}"));
    }

    time_str
}
