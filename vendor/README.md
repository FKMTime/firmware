# esp-storage 0.10.0

This is the published esp-storage 0.10.0 source from esp-rs/esp-hal commit
`63e97be553f435f29171f7ae5f37a41ad737a75a`, with its original dual MIT/Apache-2.0
license. Only `src/hardware.rs` differs: cache the first nonzero flash capacity.

On the v4 ESP32-C3, a fresh FlashStorage detects 4 MB, but another instance made
after a ROM flash read detects 0 bytes. Wi-Fi manager reads the partition table
with one instance and opens NVS with another, so the second instance rejects a
valid NVS partition as `InvalidPartitionSize`. Other flash users, including OTA,
also create fresh instances. The physical capacity is immutable; caching it
avoids repeating the hardware RDID command after ROM operations change SPI1
state. A failed detection is not cached.

Reproduced on real v4 hardware: refresh c68ff48 fails to open NVS; with this
patch, the same NVS data opens and Wi-Fi/TLS connect. No NVS erase is needed.
The simulator did not reproduce the repeated-detection failure.

Remove this override after validating an upstream release that fixes repeated
flash-size detection on the device. No upstream report or pull request was made.
