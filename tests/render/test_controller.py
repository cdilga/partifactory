#!/usr/bin/env python3
"""CDP proxy regressions: preserve upgrade bytes and bound untrusted headers."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "render_controller", Path(__file__).resolve().parents[2] / "deploy/render/controller.py")
controller = importlib.util.module_from_spec(spec)
spec.loader.exec_module(controller)


class Connection:
    def __init__(self, chunks):
        self.chunks = iter(chunks)
        self.timeout = None

    def settimeout(self, value):
        self.timeout = value

    def recv(self, _size):
        return next(self.chunks, b"")


class CdpRequestTests(unittest.TestCase):
    def test_fragmented_websocket_upgrade_keeps_headers_and_payload(self):
        connection = Connection([
            b"GET /devtools/browser/id HTTP/1.1\r\nhO",
            b"sT: partifactory-render:9222\r\nUpgrade: websocket\r\n",
            b"Connection: Upgrade\r\nOrigin: http://trusted\r\n\r\n\x81\x00",
        ])
        self.assertEqual(controller.cdp_request(connection),
            b"GET /devtools/browser/id HTTP/1.1\r\nHost: localhost:9224\r\n"
            b"Upgrade: websocket\r\nConnection: Upgrade\r\n"
            b"Origin: http://trusted\r\n\r\n\x81\x00")
        self.assertIsNone(connection.timeout)

    def test_duplicate_hosts_are_replaced_by_one_fixed_host(self):
        request = controller.cdp_request(Connection([
            b"GET /json/version HTTP/1.1\r\nHost: service\r\nhost: other\r\n\r\n"]))
        self.assertEqual(request.count(b"Host:"), 1)
        self.assertNotIn(b"service", request)
        self.assertNotIn(b"other", request)

    def test_incomplete_and_oversized_headers_are_refused(self):
        for chunks in [[b"GET /"], [b"x" * 16384]]:
            with self.subTest(chunks=len(chunks[0])):
                with self.assertRaises(ValueError):
                    controller.cdp_request(Connection(chunks))

    def test_slow_header_has_one_total_deadline(self):
        with patch.object(controller.time, "monotonic", side_effect=[0, 1, 6]):
            with self.assertRaises(TimeoutError):
                controller.cdp_request(Connection([b"GET /", b"still incomplete"]))


if __name__ == "__main__":
    unittest.main()
