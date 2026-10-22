"""No device I/O: adversarial protocol fixtures for the restricted UART reader."""
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "uart_flash_read", Path(__file__).resolve().parents[1] / "flash/uart-flash-read.py")
TOOL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TOOL)


def packet(number, payload):
    assert len(payload) == 1024
    return bytes([2, number & 255, (number & 255) ^ 255]) + payload + bytes([sum(payload) & 255])


class FakePort:
    in_waiting = 0

    def __init__(self, response=b"", initial=b"\x15" * 3):
        self.incoming = bytearray(initial)
        self.response = response
        self.writes = []

    def read(self, count):
        if not self.incoming:
            raise TimeoutError("Fixture input exhausted")
        result = bytes(self.incoming[:min(count, 17)])
        del self.incoming[:len(result)]
        return result

    def write(self, data):
        self.writes.append(data)
        if data[:1] == b"\x20":
            self.incoming.extend(self.response)
        return len(data)

    def flush(self):
        pass


class ReaderTests(unittest.TestCase):
    def test_baudcheck_accepts_in_flight_naks_without_retransmitting(self):
        port = FakePort(initial=b"\x15\x15\x06")
        TOOL.baudcheck(port)
        self.assertEqual(port.writes, [b"\x07"])
        port = FakePort(initial=b"\x15\x18")
        with self.assertRaisesRegex(TOOL.ProtocolError, "BAUDCHK response"):
            TOOL.baudcheck(port)
        self.assertEqual(port.writes, [b"\x07"])

    def test_status_pair_uses_only_fixed_reads_and_stops_on_drift_or_bad_reply(self):
        class StatusPort(FakePort):
            def __init__(self, values, bad=False):
                super().__init__()
                self.values = iter(values)
                self.bad = bad

            def write(self, data):
                self.writes.append(data)
                if data not in (b"\x21\x05\x01", b"\x21\x35\x01", b"\x21\x15\x01"):
                    raise AssertionError("Non-status command")
                self.incoming.extend(bytes([0x06 if self.bad else 0x21, next(self.values)]) + b"\x15" * 3)
                return len(data)

        for values, expected_error in [([0, 2, 0] * 2, False), ([0, 2, 0, 0, 2, 0x20], True)]:
            port = StatusPort(values)
            observations = []
            if expected_error:
                with self.assertRaisesRegex(TOOL.ProtocolError, "observations differ"):
                    TOOL.inspect_status(port, observations.append)
            else:
                result = TOOL.inspect_status(port, observations.append)
                self.assertTrue(result["matching"])
                self.assertFalse(result["bank1_assessment"]["physical_write_permission_established"])
            self.assertEqual(len(observations), 2)
            self.assertEqual(b"".join(port.writes), bytes.fromhex("210501213501211501") * 2)
        port = StatusPort([0], bad=True)
        with self.assertRaises(ValueError):
            TOOL.inspect_status(port, lambda _: None)
        self.assertEqual(port.writes, [bytes.fromhex("210501")])

    def test_status_cli_validates_evidence_before_port_open_and_never_uploads(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bounded = root / "private/bounded"
            bounded.mkdir(parents=True)
            data = b"b" * 4096
            (bounded / "flash-read.bin").write_bytes(data)
            report = {"mode": "read-small", "status": "complete", "port": "/fixture",
                      "helper_sha256": TOOL.HELPER_SHA256, "helper_upload_completed": True,
                      "result": {"bytes": 4096, "offset": 0, "sha256": TOOL.sha256(data)}}
            for name in ("serial-rx.bin", "serial-tx.bin"):
                (bounded / name).write_bytes(b"evidence")
                report[name + "_sha256"] = TOOL.sha256(b"evidence")
            (bounded / "report.json").write_text(json.dumps(report))
            fake = SimpleNamespace(open=lambda: None, close=lambda: None)
            argv = ["flash/uart-flash-read.py", "read-status", "--port", "/fixture", "--output",
                    str(root / "private/status"), "--small-report", str(bounded / "report.json"),
                    "--rom-report", str(root / "private/rom/report.json")]
            with patch.object(TOOL, "ROOT", root), patch.object(sys, "argv", argv), \
                    patch.object(TOOL.os, "umask"), patch("sys.stdout", new_callable=io.StringIO), \
                    patch.object(TOOL, "load_helper", return_value=b"helper"), \
                    patch.object(TOOL, "validate_rom_report"), \
                    patch.dict(sys.modules, {"serial": SimpleNamespace(Serial=lambda **_kw: fake)}), \
                    patch.object(TOOL, "upload_helper", side_effect=AssertionError("upload")), \
                    patch.object(TOOL, "inspect_status", return_value={"matching": True}) as inspect:
                self.assertEqual(TOOL.main(), 0)
                inspect.assert_called_once()
                (bounded / "serial-rx.bin").write_bytes(b"corrupted")
                with patch.dict(sys.modules, {"serial": SimpleNamespace(
                        Serial=lambda **_kw: self.fail("Port opened with corrupt evidence"))}):
                    with self.assertRaisesRegex(TOOL.ProtocolError, "hash mismatch"):
                        TOOL.main()
            final = json.loads((root / "private/status/report.json").read_text())
            self.assertFalse(final["helper_upload_attempted"])
            self.assertFalse(final["flash_write_or_erase_commands"])

    def test_receive_only_observation_records_fragmented_loader_output_without_tx(self):
        port = FakePort(initial=b"#Flash Download Start\r\n\x15\x15\x15\x15")
        with patch.object(TOOL.time, "monotonic", side_effect=[0, 0, .1, 2]):
            result = TOOL.observe(port, 1)
        self.assertEqual(port.writes, [])
        self.assertEqual(result["longest_nak_run"], 4)
        self.assertTrue(result["download_banner_seen"])
        self.assertEqual(result["transmitted_bytes"], 0)
        for seconds in (0, 181, float("nan")):
            with self.assertRaises(TOOL.ProtocolError):
                TOOL.observe(port, seconds)

    def test_observe_cli_never_uploads_helper_or_sends_console_commands(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            output = root / "private/observation"
            fake = SimpleNamespace(open=lambda: None, close=lambda: None)
            argv = ["flash/uart-flash-read.py", "observe", "--port", "/fixture", "--output", str(output)]
            with patch.object(TOOL, "ROOT", root), patch.object(sys, "argv", argv), \
                    patch.object(TOOL.os, "umask"), patch("sys.stdout", new_callable=io.StringIO), \
                    patch.dict(sys.modules, {"serial": SimpleNamespace(Serial=lambda **_kw: fake)}), \
                    patch.object(TOOL, "collect", return_value=b"\x15\x15\x15"), \
                    patch.object(TOOL, "load_helper", side_effect=AssertionError("helper accessed")), \
                    patch.object(TOOL, "console_query", side_effect=AssertionError("console command")), \
                    patch.object(TOOL, "upload_helper", side_effect=AssertionError("helper upload")):
                self.assertEqual(TOOL.main(), 0)
            report = json.loads((output / "report.json").read_text())
            self.assertEqual(report["status"], "complete")
            self.assertIsNone(report["helper_sha256"])
            self.assertEqual((output / "serial-tx.bin").stat().st_size, 0)

    def test_range_never_wraps_or_rounds(self):
        self.assertEqual(TOOL.read_request(0, 4096), b"\x20\0\0\0\x01\0")
        self.assertEqual(TOOL.read_request(0, TOOL.CAPACITY), b"\x20\0\0\0\0\x08")
        for offset, length in [(-4096, 4096), (1, 4096), (0, 0), (0, 1),
                               (0, TOOL.CAPACITY + 4096), (TOOL.CAPACITY, 4096)]:
            with self.subTest(offset=offset, length=length), self.assertRaises(TOOL.ProtocolError):
                TOOL.read_request(offset, length)

    def test_fragmented_exact_read_and_completion(self):
        data = bytes(range(256)) * 16
        frames = b"".join(packet(i + 1, data[i * 1024:(i + 1) * 1024]) for i in range(4))
        port = FakePort(frames + b"\x04\x15\x15\x15")
        output = io.BytesIO()
        result = TOOL.receive_flash(port, 0, 4096, output)
        self.assertEqual(output.getvalue(), data)
        self.assertEqual(result, {"bytes": 4096, "packets": 4, "duplicate_packets": 0})
        self.assertEqual(port.writes, [TOOL.read_request(0, 4096)] + [b"\x06"] * 5)

    def test_packet_counter_rollover(self):
        count = 260
        payloads = [bytes([i & 255]) * 1024 for i in range(count)]
        frames = b"".join(packet(i + 1, p) for i, p in enumerate(payloads))
        output = io.BytesIO()
        result = TOOL.receive_flash(FakePort(frames + b"\x04\x15\x15\x15"),
                                    0, count * 1024, output)
        self.assertEqual(output.getvalue(), b"".join(payloads))
        self.assertEqual(result["packets"], count)

    def test_duplicate_requires_identical_previous_payload(self):
        payload = b"x" * 1024
        frames = packet(1, payload) + packet(1, payload)
        frames += b"".join(packet(i, payload) for i in range(2, 5))
        out = io.BytesIO()
        result = TOOL.receive_flash(FakePort(frames + b"\x04\x15\x15\x15"), 0, 4096, out)
        self.assertEqual(result["duplicate_packets"], 1)
        self.assertEqual(out.getvalue(), payload * 4)
        with self.assertRaises(TOOL.ProtocolError):
            TOOL.receive_flash(FakePort(packet(1, payload) + packet(1, b"z" * 1024)),
                               0, 4096, io.BytesIO())

    def test_bad_checksum_complement_order_cancel_and_early_end_abort(self):
        good = packet(1, b"x" * 1024)
        bad_sum = good[:-1] + bytes([good[-1] ^ 1])
        bad_complement = good[:2] + bytes([good[2] ^ 1]) + good[3:]
        for bad in [bad_sum, bad_complement, packet(2, b"x" * 1024), b"\x18\x18", b"\x04"]:
            port = FakePort(bad)
            with self.subTest(first_byte=bad[0]), self.assertRaises(TOOL.ProtocolError):
                TOOL.receive_flash(port, 0, 4096, io.BytesIO())
            self.assertEqual(port.writes, [TOOL.read_request(0, 4096)])

    def test_truncated_or_excessive_read_is_not_success(self):
        payload = b"x" * 1024
        with self.assertRaises(TimeoutError):
            TOOL.receive_flash(FakePort(packet(1, payload)[:500]), 0, 4096, io.BytesIO())
        extra = b"".join(packet(i, payload) for i in range(1, 6))
        with self.assertRaises(TOOL.ProtocolError):
            TOOL.receive_flash(FakePort(extra), 0, 4096, io.BytesIO())

    def test_mutated_helper_and_out_of_range_upload_are_refused(self):
        with self.assertRaises(TOOL.ProtocolError):
            TOOL.upload_helper(FakePort(), bytes(TOOL.HELPER_BYTES))
        helper = bytes(i & 255 for i in range(TOOL.HELPER_BYTES))
        with patch.object(TOOL, "HELPER_SHA256", TOOL.sha256(helper)):
            for index in [-1, 5]:
                with self.assertRaises(TOOL.ProtocolError):
                    TOOL.upload_frame(helper, index)
            frame = TOOL.upload_frame(helper, 4)
            self.assertEqual(frame[:3], b"\x02\x05\xfa")
            self.assertEqual(frame[3:7], (0x83000).to_bytes(4, "little"))
            self.assertEqual(frame[7:7 + 592], helper[4096:])
            self.assertEqual(frame[7 + 592:-1], b"\xff" * (1024 - 592))
            self.assertEqual(frame[-1], sum(frame[3:-1]) & 255)

    def test_upload_protocol_only_addresses_pinned_ram(self):
        helper = bytes(i & 255 for i in range(TOOL.HELPER_BYTES))

        class UploadPort(FakePort):
            def write(self, data):
                self.writes.append(data)
                if data == b"\x07" or data[:1] == b"\x02":
                    self.incoming.extend(b"\x06")
                elif data == b"\x04":
                    self.incoming.extend(b"\x06\x15\x15\x15")
                else:
                    raise AssertionError("Unexpected upload write")
                return len(data)

        port = UploadPort()
        with patch.object(TOOL, "HELPER_SHA256", TOOL.sha256(helper)):
            TOOL.upload_helper(port, helper)
        self.assertEqual(port.writes[0], b"\x07")
        self.assertEqual(port.writes[-1], b"\x04")
        self.assertEqual(len(port.writes), 7)
        self.assertEqual([int.from_bytes(frame[3:7], "little") for frame in port.writes[1:-1]],
                         [0x82000, 0x82400, 0x82800, 0x82C00, 0x83000])

    def test_console_forbids_flash_mmio_writes_and_unexpected_core(self):
        for command in [b"EW 00001298 0\r\n", b"DW 08000000\r\n", b"DW 48000600\r\n",
                        b"reboot uartburn\r\n", b"FLASH read 0 4096\r\n"]:
            port = FakePort(initial=b"")
            with self.assertRaises(TOOL.ProtocolError):
                TOOL.console_query(port, command)
            self.assertEqual(port.writes, [])
        with patch.object(TOOL, "collect", return_value=b"unexpected help"):
            port = FakePort(initial=b"")
            with self.assertRaises(TOOL.ProtocolError):
                TOOL.inspect_rom(port, bytes(54144))
            self.assertEqual(port.writes, [b"?\r\n"])

    def test_fault_marker_stops_before_further_commands(self):
        with patch.object(TOOL, "collect", return_value=b"Hard Fault Patch (Non-secure)"):
            port = FakePort(initial=b"")
            with self.assertRaises(TOOL.ProtocolError):
                TOOL.console_query(port, b"?\r\n")
            self.assertEqual(port.writes, [b"?\r\n"])

    def test_rom_inspection_verifies_core_before_reads_and_returns_to_km4(self):
        candidate = bytes((i * 3) & 255 for i in range(54144))
        high = b"HELP [0e11df78] switch shell to KM0"
        low = b"TEST COMMAND MODE HELP 12 switch shell to KM4 HSSDM32K CTC FWDBG TEST COMMAND MODE END  c"
        responses = [b"", high, b"", low]
        for base in TOOL.ROM_POINTS:
            for address in range(base & ~3, (base & ~3) + 16, 4):
                value = int.from_bytes(candidate[address:address + 4], "little")
                responses.append(f"{address:08X}: {value:08X}\n".encode())
        responses.extend([b"", high])
        port = FakePort(initial=b"")
        with patch.object(TOOL, "collect", side_effect=responses):
            result = TOOL.inspect_rom(port, {"fixture": candidate})
        self.assertTrue(result["returned_to_km4"])
        self.assertEqual(len(result["sampled_words"]), 68)
        self.assertEqual(port.writes[:3], [b"?\r\n", b">>\r\n", b"?\r\n"])
        self.assertEqual(port.writes[-2:], [b">>\r\n", b"?\r\n"])
        # A high-core help response after switching must block every ROM read.
        port = FakePort(initial=b"")
        with patch.object(TOOL, "collect", side_effect=[b"", high, b"", high]):
            with self.assertRaises(TOOL.ProtocolError):
                TOOL.inspect_rom(port, {"fixture": candidate})
        self.assertEqual(port.writes, [b"?\r\n", b">>\r\n", b"?\r\n"])

    def test_rom_mismatch_stops_without_upload_or_core_switch_fallback(self):
        port = FakePort(initial=b"")
        replies = [b"", b"0e11df78 switch shell to KM0", b"",
                   b"TEST COMMAND MODE HELP 12 switch shell to KM4 HSSDM32K CTC FWDBG TEST COMMAND MODE END  c",
                   b"00001298: FFFFFFFF\n"]
        with patch.object(TOOL, "collect", side_effect=replies):
            with self.assertRaises(TOOL.ProtocolError):
                TOOL.inspect_rom(port, {"fixture": bytes(54144)})
        self.assertEqual(port.writes[-1], b"DW 00001298\r\n")
        self.assertTrue(all(not data.startswith(b"\x02") for data in port.writes))

    def test_full_read_continues_helper_without_overwriting_its_ram(self):
        # Exercise CLI orchestration as well as packet code: a full read must
        # never re-upload over the still-executing bounded-read helper.
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            private = root / "private"
            bounded = private / "bounded"
            bounded.mkdir(parents=True)
            data = bytes(range(256)) * 16
            (bounded / "flash-read.bin").write_bytes(data)
            report = {"mode": "read-small", "status": "complete", "port": "/fixture",
                      "helper_sha256": TOOL.HELPER_SHA256, "helper_upload_completed": True,
                      "result": {"bytes": 4096, "sha256": TOOL.sha256(data)}}
            (bounded / "report.json").write_text(json.dumps(report))
            fake = SimpleNamespace(open=lambda: None, close=lambda: None)

            def receive(_port, offset, length, output, _progress):
                self.assertEqual((offset, length), (0, TOOL.CAPACITY))
                output.write(b"x" * length)
                return {"bytes": length, "packets": length // 1024, "duplicate_packets": 0}

            argv = ["flash/uart-flash-read.py", "read-full", "--port", "/fixture", "--output",
                    str(private / "full"), "--small-report", str(bounded / "report.json"),
                    "--rom-report", str(private / "rom" / "report.json")]
            with patch.object(TOOL, "ROOT", root), patch.object(sys, "argv", argv), \
                    patch.object(TOOL.os, "umask"), patch("sys.stdout", new_callable=io.StringIO), \
                    patch.object(TOOL, "load_helper", return_value=b"pinned helper fixture"), \
                    patch.object(TOOL, "validate_rom_report"), \
                    patch.dict(sys.modules, {"serial": SimpleNamespace(Serial=lambda **_kw: fake)}), \
                    patch.object(TOOL, "receive_flash", side_effect=receive), \
                    patch.object(TOOL, "upload_helper", side_effect=AssertionError("self-overwrite")):
                self.assertEqual(TOOL.main(), 0)
            final = json.loads((private / "full" / "report.json").read_text())
            self.assertFalse(final["helper_upload_attempted"])
            self.assertEqual(final["status"], "complete")
            self.assertEqual((private / "full" / "serial-tx.bin").stat().st_size, 0)


if __name__ == "__main__":
    unittest.main()
