#!/usr/bin/env python3
"""Espejo local de sitios reales, solo para desarrollo en entornos SIN salida
directa a internet (por ejemplo una sesion de Claude Code en la nube).

El motor no sabe usar un proxy HTTP ni confia en su certificado, asi que
ahi cualquier https:// falla con "client error (Connect)". Eso es el
entorno, no un bug del motor. Este script abre un puerto local por sitio y
reenvia cada GET al sitio real usando la red del sistema (que si pasa por
el proxy). En un PC normal no hace falta: el motor llega solo a internet.

Uso:
  python3 scripts/dev/site_mirror.py 9001=es.wikipedia.org 9003=news.ycombinator.com
  python3 scripts/dev/engine_driver.py http://127.0.0.1:9001/wiki/Navegador_web

Limites: solo GET. Los recursos de OTROS dominios (imagenes de un CDN) no
pasan por el espejo y fallaran.
"""
import http.server
import socketserver
import sys
import threading
import time
import urllib.error
import urllib.request

USER_AGENT = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36"


def make_handler(host):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, fmt, *args):
            sys.stderr.write("[%s] %s\n" % (host, fmt % args))

        def do_GET(self):
            request = urllib.request.Request(
                "https://" + host + self.path,
                headers={"User-Agent": USER_AGENT, "Accept": "*/*", "Accept-Language": "es,en;q=0.8"},
            )
            try:
                with urllib.request.urlopen(request, timeout=40) as upstream:
                    body, status = upstream.read(), upstream.status
                    content_type = upstream.headers.get("Content-Type", "application/octet-stream")
            except urllib.error.HTTPError as error:
                body, status = error.read(), error.code
                content_type = error.headers.get("Content-Type", "text/plain")
            except Exception as error:  # red caida, DNS, timeout...
                body, status, content_type = str(error).encode(), 502, "text/plain"
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)

    return Handler


class ThreadedServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    for spec in sys.argv[1:]:
        port, host = spec.split("=", 1)
        server = ThreadedServer(("127.0.0.1", int(port)), make_handler(host))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        print(f"espejo http://127.0.0.1:{port}/ -> https://{host}/", flush=True)
    while True:
        time.sleep(3600)


if __name__ == "__main__":
    main()
