#!/usr/bin/env python3
"""BLE "phone" that provisions Wi-Fi over wifimanager's GATT service (Bumble).

Start this first, then the firmware in provisioning mode:
    ~/tmp/esp-sim-patches/venv/bin/python sim/ble_phone.py
    SIM_PROVISION=1 SIM_BLE=tcp:localhost:9544 sim/run.sh
"""

import asyncio
import json
import sys

from bumble import hci
from bumble.controller import Controller
from bumble.device import Device, Peer
from bumble.host import Host
from bumble.link import LocalLink
from bumble.transport import open_transport

SERVICE = "F254A578-EF88-4372-B5F5-5ECF87E65884"
SETUP = "BCD7E573-B0B2-4775-83C0-ACBF3AAF210C"
SCAN_RES = "22E997B5-0AC5-475D-AB6C-9C9568B6620A"
SETUP_JSON = {"ssid": "myssid", "psk": "mypassword",
              "data": {"mdns": False, "ws_url": "ws://192.168.4.1:8080"}}


def ack_ok(self, command):
    """Bumble's virtual controller ignores some LL privacy commands; ack them as success."""
    params = hci.HCI_GenericReturnParameters(data=bytes([0]))
    if isinstance(command, hci.HCI_SyncCommand):
        return params
    self.send_hci_packet(hci.HCI_Command_Complete_Event(
        num_hci_command_packets=1, command_opcode=command.op_code, return_parameters=params))
    return None


for name in ("on_hci_le_set_privacy_mode_command", "on_hci_le_clear_resolving_list_command",
             "on_hci_le_add_device_to_resolving_list_command",
             "on_hci_le_remove_device_from_resolving_list_command"):
    if not hasattr(Controller, name):
        setattr(Controller, name, ack_ok)


async def main():
    link = LocalLink()
    emu = await open_transport("tcp-server:_:9544")
    Controller("emu", host_source=emu.source, host_sink=emu.sink, link=link,
               public_address="00:11:22:33:44:55")

    phone_ctrl = Controller("phone", link=link, public_address="AA:BB:CC:DD:EE:FF")
    host = Host()
    host.controller, phone_ctrl.host = phone_ctrl, host
    phone = Device(name="phone", host=host)
    await phone.power_on()
    print("[phone] waiting for an FKM-… advertisement", flush=True)

    found = asyncio.get_running_loop().create_future()

    def on_adv(adv):
        name = adv.data.get(0x09, b"")
        name = name.decode(errors="replace") if isinstance(name, bytes) else str(name)
        if name.startswith("FKM-") and not found.done():
            print(f"[phone] found {name} at {adv.address}", flush=True)
            found.set_result(adv.address)

    phone.on("advertisement", on_adv)
    await phone.start_scanning()
    address = await asyncio.wait_for(found, 120)
    await phone.stop_scanning()

    conn = await phone.connect(address, timeout=15)
    peer = Peer(conn)
    await peer.discover_services()
    service = next(s for s in peer.services if str(s.uuid).upper() == SERVICE)
    await service.discover_characteristics()
    chars = {str(c.uuid).upper(): c for c in service.characteristics}
    print("[phone] scan results:", bytes(await chars[SCAN_RES].read_value()).decode(errors="replace"),
          flush=True)

    payload = json.dumps(SETUP_JSON).encode() + b"\0"
    for i in range(0, len(payload), 20):  # plain writes, accumulated until the \0
        await chars[SETUP].write_value(payload[i:i + 20], with_response=True)
    print(f"[phone] setup written ({len(payload)} bytes)", flush=True)
    await asyncio.sleep(3)
    try:
        await conn.disconnect()
    except Exception:
        pass
    print("[phone] done", flush=True)


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
