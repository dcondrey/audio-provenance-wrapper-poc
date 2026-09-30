import importlib.util
import ssl
import sys
import tempfile
import threading
import unittest
from http.server import ThreadingHTTPServer
from pathlib import Path


class TmpMixin(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.tmp = Path(self._tmp.name)

    def write(self, name: str, data) -> Path:
        path = self.tmp / name
        path.write_bytes(data if isinstance(data, bytes) else data.encode())
        return path


def serve(handler, context: ssl.SSLContext | None = None) -> ThreadingHTTPServer:
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    server.daemon_threads = True
    if context is not None:
        server.socket = context.wrap_socket(server.socket, server_side=True)
    # the 0.5 s default poll interval is paid again in every shutdown()
    threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True).start()
    return server


def load_script(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module  # @dataclass resolves annotations through sys.modules
    spec.loader.exec_module(module)
    return module
