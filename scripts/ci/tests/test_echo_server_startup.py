"""Loopback echo fixtures must not depend on the host's reverse DNS service."""

from __future__ import annotations

import contextlib
import http.client
import importlib.util
import io
import json
import shutil
import socket
import socketserver
import ssl
import sys
import threading
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[3]


def load_fixture(name: str):
    spec = importlib.util.spec_from_file_location(
        name, ROOT / "e2e-tests/mock_servers" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


HTTP = load_fixture("http_echo_server")
HTTPS = load_fixture("https_echo_server")
FIXTURES = (HTTP, HTTPS)


class EchoServerBindTests(unittest.TestCase):
    def test_numeric_loopback_bind_never_calls_reverse_dns(self) -> None:
        for module in FIXTURES:
            with self.subTest(fixture=module.__name__), mock.patch(
                "socket.getfqdn", side_effect=AssertionError("reverse DNS must not run")
            ) as lookup:
                with module.ThreadedHTTPServer(("127.0.0.1", 0), module.EchoHandler) as server:
                    self.assertEqual(server.server_name, "127.0.0.1")
                    self.assertGreater(server.server_port, 0)
                    self.assertEqual(server.server_address, server.socket.getsockname())
                    self.assertEqual(server.server_address[1], server.server_port)
                    self.assertTrue(server.daemon_threads)
                    if hasattr(socket, "SO_REUSEADDR"):
                        self.assertEqual(server.socket.getsockopt(
                            socket.SOL_SOCKET, socket.SO_REUSEADDR), 1)
                lookup.assert_not_called()

    def test_numeric_loopback_variants_keep_bound_socket_metadata(self) -> None:
        # Mock only the OS bind, so IPv6 availability does not affect this test.
        for module in FIXTURES:
            for host, bound in (("127.0.0.2", ("127.0.0.2", 32123)),
                                ("::1", ("::1", 32124, 0, 0))):
                with self.subTest(fixture=module.__name__, host=host):
                    with module.ThreadedHTTPServer(
                        (host, 0), module.EchoHandler, bind_and_activate=False
                    ) as server:
                        def bind(instance):
                            instance.server_address = bound
                        with mock.patch.object(socketserver.TCPServer, "server_bind",
                                               autospec=True, side_effect=bind) as tcp_bind, \
                                mock.patch("socket.getfqdn", side_effect=AssertionError(
                                    "reverse DNS must not run")) as lookup:
                            server.server_bind()
                        tcp_bind.assert_called_once_with(server)
                        lookup.assert_not_called()
                        self.assertEqual(server.server_name, bound[0])
                        self.assertEqual(server.server_port, bound[1])

    def test_hostname_wildcard_and_non_loopback_keep_httpserver_semantics(self) -> None:
        for module in FIXTURES:
            for host, bound_host in (("localhost", "127.0.0.1"),
                                     ("example.test", "192.0.2.12"),
                                     ("192.0.2.12", "192.0.2.12"),
                                     ("0.0.0.0", "0.0.0.0"),
                                     ("", "0.0.0.0")):
                with self.subTest(fixture=module.__name__, host=host):
                    with module.ThreadedHTTPServer(
                        (host, 0), module.EchoHandler, bind_and_activate=False
                    ) as server:
                        def bind(instance):
                            instance.server_address = (bound_host, 32125)
                        with mock.patch.object(socketserver.TCPServer, "server_bind",
                                               autospec=True, side_effect=bind) as tcp_bind, \
                                mock.patch("socket.getfqdn", return_value="fixture.test") as lookup:
                            server.server_bind()
                        tcp_bind.assert_called_once_with(server)
                        lookup.assert_called_once_with(bound_host)
                        self.assertEqual(server.server_name, "fixture.test")
                        self.assertEqual(server.server_port, 32125)

    def test_bind_failure_propagates_without_dns_or_port_substitution(self) -> None:
        with socket.socket() as occupied:
            occupied.bind(("127.0.0.1", 0))
            occupied.listen()
            for module in FIXTURES:
                with self.subTest(fixture=module.__name__), mock.patch(
                    "socket.getfqdn", side_effect=AssertionError("reverse DNS must not run")
                ) as lookup:
                    with self.assertRaises(OSError):
                        module.ThreadedHTTPServer(occupied.getsockname(), module.EchoHandler)
                    lookup.assert_not_called()

    def test_ephemeral_loopback_health_and_echo_over_http_and_tls(self) -> None:
        # Exercise real binding/listening, TLS wrapping and request handlers.
        # No system proxy, certificate installation or external endpoint is used.
        for module in FIXTURES:
            with self.subTest(fixture=module.__name__), contextlib.ExitStack() as stack:
                stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
                lookup = stack.enter_context(mock.patch(
                    "socket.getfqdn", side_effect=AssertionError("reverse DNS must not run")))
                server = stack.enter_context(module.ThreadedHTTPServer(
                    ("127.0.0.1", 0), module.EchoHandler))
                if module is HTTPS:
                    cert, key, cert_dir = HTTPS.generate_self_signed_cert()
                    stack.callback(shutil.rmtree, cert_dir)
                    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
                    tls.load_cert_chain(cert, key)
                    server.socket = tls.wrap_socket(server.socket, server_side=True)
                thread = threading.Thread(target=server.serve_forever, daemon=True)
                thread.start()
                stack.callback(thread.join, 5)
                stack.callback(server.shutdown)
                if module is HTTPS:
                    # Trust only this ephemeral fixture certificate, in memory.
                    # The openssl fallback does not provide a numeric-host SAN.
                    client_tls = ssl.create_default_context(cafile=cert)
                    client_tls.check_hostname = False
                    connection = http.client.HTTPSConnection(
                        "127.0.0.1", server.server_port, timeout=3, context=client_tls)
                else:
                    connection = http.client.HTTPConnection(
                        "127.0.0.1", server.server_port, timeout=3)
                stack.callback(connection.close)
                connection.request("GET", "/health")
                response = connection.getresponse()
                self.assertEqual(response.status, 200)
                self.assertEqual(response.read(), b"ok")
                self.assertEqual(response.getheader("X-Echo-Server"),
                                 "bifrost-test-https" if module is HTTPS else "bifrost-test")
                connection.request("GET", "/startup-echo?value=1")
                response = connection.getresponse()
                self.assertEqual(response.status, 200)
                body = json.loads(response.read())
                self.assertEqual(body["server"]["port"], server.server_port)
                self.assertEqual(body["server"]["address"], f"127.0.0.1:{server.server_port}")
                self.assertEqual(body["request"]["path"], "/startup-echo?value=1")
                if module is HTTPS:
                    self.assertTrue(body["server"]["tls"]["version"].startswith("TLS"))
                lookup.assert_not_called()


class HttpsStartupPhaseTests(unittest.TestCase):
    def run_main(self, fail_at: str | None = None) -> tuple[str, list[str]]:
        events = []
        output = io.StringIO()

        def stage(name, result):
            def run(*_args, **_kwargs):
                events.append(name)
                if fail_at == name:
                    raise RuntimeError(f"failed at {name}")
                return result
            return run

        server = mock.MagicMock()
        server.__enter__.return_value = server
        server.serve_forever.side_effect = stage("serve", None)
        tls = mock.Mock()
        tls.load_cert_chain.side_effect = stage("load", None)
        tls.wrap_socket.side_effect = stage("wrap", mock.sentinel.tls_socket)
        with contextlib.redirect_stdout(output), \
                mock.patch.object(sys, "argv", ["https_echo_server.py", "32126"]), \
                mock.patch.object(HTTPS, "generate_self_signed_cert", side_effect=stage(
                    "cert", ("cert.pem", "key.pem", "fixture-cert-dir"))), \
                mock.patch.object(HTTPS.ssl, "SSLContext", return_value=tls), \
                mock.patch.object(HTTPS, "ThreadedHTTPServer", side_effect=stage("bind", server)), \
                mock.patch("shutil.rmtree"):
            if fail_at:
                with self.assertRaisesRegex(RuntimeError, f"failed at {fail_at}"):
                    HTTPS.main()
            else:
                HTTPS.main()
        return output.getvalue(), events

    def test_startup_phases_are_visible_before_failure_and_never_claim_ready(self) -> None:
        phases = ("cert", "load", "bind", "wrap")
        messages = ("Generating self-signed certificate", "Loading TLS certificate",
                    "Binding HTTPS listener", "Wrapping HTTPS listener with TLS")
        for index, phase in enumerate(phases):
            with self.subTest(phase=phase):
                output, events = self.run_main(phase)
                self.assertEqual(events, list(phases[:index + 1]))
                for message in messages[:index + 1]:
                    self.assertIn(message, output)
                for message in messages[index + 1:]:
                    self.assertNotIn(message, output)
                self.assertNotIn("READY", output.splitlines())

    def test_ready_remains_after_successful_bind_and_tls_wrap(self) -> None:
        output, events = self.run_main()
        self.assertEqual(events, ["cert", "load", "bind", "wrap", "serve"])
        self.assertEqual(output.splitlines().count("READY"), 1)
        self.assertLess(output.index("Wrapping HTTPS listener with TLS"), output.index("READY"))


if __name__ == "__main__":
    unittest.main()
