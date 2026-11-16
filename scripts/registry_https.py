#!/usr/bin/env python3
"""A local static HTTPS server for registry tests (Phase 2 E1, ADR-011).

Serves one directory read-only over TLS on 127.0.0.1 with a given
certificate, the way GitHub Pages serves the registry: GET and HEAD of
files, 404 for anything missing. bb fetches https:// only, so the test
registry is HTTPS too; curl trusts the test certificate through
CURL_CA_BUNDLE.

usage: registry_https.py <root> <cert.pem> <key.pem> <port file>
  Binds an ephemeral port and writes it to <port file> once it listens.
  A request log goes to stderr (one line per request).
"""

import functools
import http.server
import os
import ssl
import sys


class Handler(http.server.SimpleHTTPRequestHandler):
    def list_directory(self, path):  # no directory listings
        self.send_error(404)
        return None

    def log_message(self, fmt, *args):
        sys.stderr.write("registry_https: " + (fmt % args) + "\n")


def main():
    if len(sys.argv) != 5:
        sys.exit(__doc__)
    root, cert, key, port_file = sys.argv[1:]
    handler = functools.partial(Handler, directory=os.path.abspath(root))
    httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(cert, key)
    httpd.socket = ctx.wrap_socket(httpd.socket, server_side=True)
    tmp = port_file + ".tmp"
    with open(tmp, "w") as f:
        f.write(f"{httpd.server_address[1]}\n")
    os.replace(tmp, port_file)
    httpd.serve_forever()


if __name__ == "__main__":
    main()
