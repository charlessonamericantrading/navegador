#!/usr/bin/env python3
"""Mide el motor Rust (engine_server) de punta a punta por su protocolo NDJSON.

Herramienta de desarrollo, no forma parte del producto. Arranca el motor,
carga una pagina, guarda la captura y mide cuanto tarda cada operacion
(navegar, scroll, clic en zona vacia, redimensionar). Es la forma de
comprobar cada tarea de PLAN_SONNET.md con numeros en vez de a ojo.

Uso:
  python3 scripts/dev/engine_driver.py https://es.wikipedia.org/wiki/Navegador_web
  python3 scripts/dev/engine_driver.py --fixture css-basics-fixture.html --out css.png
  python3 scripts/dev/engine_driver.py --fixture js-basics-fixture.html --texts

Requiere el motor compilado en release:
  cargo build --release --manifest-path engine/Cargo.toml -p engine-core --bin engine_server
"""
import argparse
import base64
import functools
import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
EXE = "engine_server.exe" if os.name == "nt" else "engine_server"
DEFAULT_ENGINE = os.path.join(ROOT, "engine", "target", "release", EXE)
FIXTURE_DIR = os.path.join(ROOT, "engine", "tests")


class Engine:
    def __init__(self, path):
        if not os.path.exists(path):
            sys.exit(f"No existe {path}. Compila primero el motor en release (ver cabecera de este archivo).")
        # Perfil temporal: sin esto, medir escribiria cookies y localStorage
        # en el perfil real de quien ejecuta el script.
        env = dict(os.environ)
        env.setdefault("NAVEGADOR_IA_PROFILE_DIR", tempfile.mkdtemp(prefix="navegador-ia-medida-"))
        self.proc = subprocess.Popen([path], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=sys.stderr, env=env)
        self.count = 0
        self.proc.stdout.readline()  # mensaje "ready"

    def request(self, timeout, **payload):
        """Envia una peticion y espera su respuesta. Devuelve un dict con
        `seconds` (tiempo real) y `bytes` (tamaño de la respuesta)."""
        self.count += 1
        payload["id"] = f"r{self.count}"
        started = time.time()
        self.proc.stdin.write((json.dumps(payload) + "\n").encode())
        self.proc.stdin.flush()
        box = {}
        reader = threading.Thread(target=lambda: box.update(line=self.proc.stdout.readline()), daemon=True)
        reader.start()
        reader.join(timeout)
        elapsed = round(time.time() - started, 2)
        if reader.is_alive():
            return {"type": "TIMEOUT", "seconds": elapsed}
        if not box.get("line"):
            return {"type": "MOTOR_CERRADO", "seconds": elapsed, "exit_code": self.proc.poll()}
        response = json.loads(box["line"])
        response["seconds"] = elapsed
        response["bytes"] = len(box["line"])
        return response

    def close(self):
        self.proc.kill()


def serve_fixtures():
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=FIXTURE_DIR)
    handler.log_message = lambda *args: None
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server.server_address[1]


def accessible_texts(engine, timeout):
    """Texto de cada nodo del arbol de accesibilidad, en orden de documento.

    `elements` (la lista de la respuesta `state`) solo trae lo interactivo
    desde la tarea 3.3; para leer el texto de una pagina hay que mirar el
    arbol de accesibilidad.
    """
    response = engine.request(timeout, type="get_accessibility_tree")
    texts = []

    def walk(node):
        if node.get("name"):
            texts.append(node["name"])
        for child in node.get("children", []):
            walk(child)

    if response.get("type") == "accessibility_tree":
        walk(response["tree"]["root"])
    return texts


def brief(response):
    keys = ("type", "seconds", "bytes", "url", "title", "message")
    out = {k: response[k] for k in keys if k in response}
    if response.get("elements") is not None:
        out["elements"] = len(response["elements"])
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("url", nargs="?", help="URL http(s) a cargar")
    parser.add_argument("--fixture", help="archivo dentro de engine/tests/ (se sirve en local)")
    parser.add_argument("--out", help="ruta donde guardar la captura PNG")
    parser.add_argument("--width", type=int, default=1280)
    parser.add_argument("--height", type=int, default=800)
    parser.add_argument("--timeout", type=int, default=120, help="segundos maximos por operacion")
    parser.add_argument("--texts", action="store_true", help="imprime el texto de cada <p> (para js-basics-fixture)")
    parser.add_argument("--engine", default=os.environ.get("ENGINE", DEFAULT_ENGINE))
    args = parser.parse_args()

    if args.fixture:
        url = f"http://127.0.0.1:{serve_fixtures()}/{args.fixture}"
    elif args.url:
        url = args.url
    else:
        parser.error("indica una URL o --fixture")

    engine = Engine(args.engine)
    failed = False
    try:
        engine.request(args.timeout, type="resize", width=args.width, height=args.height)
        nav = engine.request(args.timeout, type="navigate", url=url)
        print("navigate     ", brief(nav))
        if nav.get("type") != "state":
            return 1
        if args.out and nav.get("screenshot"):
            with open(args.out, "wb") as f:
                f.write(base64.b64decode(nav["screenshot"]))
            print("captura      ", args.out)
        if args.texts:
            seen = set()
            for text in accessible_texts(engine, args.timeout):
                if text not in seen:
                    seen.add(text)
                    print("   ", text[:140])
        steps = [
            ("scroll       ", dict(type="scroll", dx=0, dy=600)),
            ("click vacio  ", dict(type="click", x=2, y=2)),
            ("resize       ", dict(type="resize", width=args.width - 200, height=args.height - 100)),
        ]
        for label, payload in steps:
            response = engine.request(args.timeout, **payload)
            print(label, brief(response))
            if response.get("type") in ("TIMEOUT", "MOTOR_CERRADO"):
                failed = True
                break
    finally:
        engine.close()
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
