import importlib.util
import json
from pathlib import Path
import sys
import unittest

FIRMWARE_TOOLS = Path(__file__).resolve().parents[2] / 'firmware/tools'
sys.path.insert(0, str(FIRMWARE_TOOLS))
SPEC = importlib.util.spec_from_file_location('firmware_build', FIRMWARE_TOOLS / 'build.py')
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)

GOOD = {'version': 1, 'broker': '10.42.103.1', 'port': 1883, 'device_id': 'dev1', 'client_id': 'wellfed-fw-dev1',
        'username': 'wellfed-fw', 'password': '0123456789abcdef' * 4, 'namespace': 'wellfed/1/dev1/'}


class TransportHeader(unittest.TestCase):
    def header(self, **change):
        return BUILD.transport_header(json.dumps({**GOOD, **change}).encode())

    def test_good_credentials_become_defines(self):
        text = self.header()
        self.assertIn('#define WELLFED_MQTT_BROKER 0x0a2a6701U\n', text)
        self.assertIn('#define WELLFED_MQTT_PORT 1883U\n', text)
        self.assertIn('#define WELLFED_MQTT_CLIENT_ID "wellfed-fw-dev1"\n', text)
        self.assertIn('#define WELLFED_MQTT_NAMESPACE "wellfed/1/dev1/"\n', text)
        self.assertIn(f'#define WELLFED_MQTT_PASSWORD "{GOOD["password"]}"\n', text)

    def test_anything_the_server_would_not_write_is_refused(self):
        for change in ({'version': 2}, {'extra': 1}, {'broker': '0.0.0.0'}, {'broker': '224.0.0.1'},
                       {'broker': '255.255.255.255'}, {'port': 0}, {'port': '1883'}, {'port': True},
                       {'client_id': 'other'}, {'username': 'device'}, {'namespace': 'dl/PLAF103/dev1/'},
                       {'device_id': 'dev/1'}, {'password': 'short'}, {'password': 'X' * 64},
                       {'password': '"' + 'a' * 63}):
            with self.subTest(change=change), self.assertRaises((RuntimeError, ValueError)):
                self.header(**change)
        with self.assertRaises(RuntimeError):
            BUILD.transport_header(json.dumps({k: v for k, v in GOOD.items() if k != 'port'}).encode())


if __name__ == '__main__':
    unittest.main()
