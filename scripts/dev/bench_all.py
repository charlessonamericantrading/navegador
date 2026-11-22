#!/usr/bin/env python3
"""Banco de medidas del motor: una tabla con lo que de verdad tarda cada pagina.

Herramienta de desarrollo (tarea 0.4 de PLAN_SONNET.md). Pasa por los dos
fixtures locales y por cuatro paginas reales, mide navegar, scroll, clic en
zona vacia y redimensionar, y sale con error si alguna operacion supera su
limite. Sirve para comprobar con numeros que un cambio ayuda o estropea.

No descarga ni guarda paginas de terceros: las carga en vivo, igual que lo
haria el navegador. Las cifras de paginas reales cambian con el tiempo; lo
que se compara son los ordenes de magnitud, no los decimales.

Uso:
  python3 scripts/dev/bench_all.py                # PC con internet
  python3 scripts/dev/bench_all.py --mirror       # sesion en la nube, sin internet directo
  python3 scripts/dev/bench_all.py --strict       # exige los objetivos de la Fase 1
  python3 scripts/dev/bench_all.py --only wikipedia --out-dir capturas

Limites (segundos):
  por defecto  cualquier operacion <= 10         (que nada se cuelgue)
  --strict     cargar <= 3, clic <= 0.3, scroll <= 0.2, redimensionar <= 1

Requiere el motor compilado en release (ver cabecera de engine_driver.py) o
la variable ENGINE apuntando al binario.
"""
import argparse
import os
import re
import sys
import threading
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import engine_driver as driver  # noqa: E402
import site_mirror  # noqa: E402

PAGES = [
    ("wikipedia", "https://es.wikipedia.org/wiki/Navegador_web"),
    ("hackernews", "https://news.ycombinator.com/"),
    ("duckduckgo", "https://html.duckduckgo.com/html/?q=rust+browser+engine"),
    ("google", "https://www.google.com/"),
]
FIXTURES = [
    ("fixture-css", "css-basics-fixture.html"),
    ("fixture-js", "js-basics-fixture.html"),
]

OPERATIONS = ("cargar", "scroll", "clic", "resize")
HARD_LIMIT = {"cargar": 10.0, "scroll": 10.0, "clic": 10.0, "resize": 10.0}
GOAL_LIMIT = {"cargar": 3.0, "scroll": 0.2, "clic": 0.3, "resize": 1.0}
# Un tiempo de espera mayor que cualquier limite: si se agota, el motor esta colgado.
OPERATION_TIMEOUT = 60

JS_CHECKS = 12
JS_OK = re.compile(r"^[a-l]: OK")


def start_mirrors(pages):
    """Un espejo local por host. Devuelve {host: puerto}."""
    ports = {}
    for _name, url in pages:
        host = urllib.parse.urlsplit(url).netloc
        if host in ports:
            continue
        server = site_mirror.ThreadedServer(("127.0.0.1", 0), site_mirror.make_handler(host))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        ports[host] = server.server_address[1]
    return ports


def through_mirror(url, ports):
    parts = urllib.parse.urlsplit(url)
    return urllib.parse.urlunsplit(("http", f"127.0.0.1:{ports[parts.netloc]}", parts.path, parts.query, ""))


def count_js_ok(engine, timeout):
    """Cuantas de las 12 comprobaciones del fixture JS terminaron en OK. El
    texto se lee del arbol de accesibilidad: `elements` ya solo trae lo
    interactivo y los <p> del fixture no lo son."""
    oks = {text.strip()[0] for text in driver.accessible_texts(engine, timeout) if JS_OK.match(text.strip())}
    return len(oks)


def measure(engine_path, url, timeout, out_png=None, count_js=False):
    """Una pagina, un motor nuevo. Devuelve ({operacion: segundos|None}, nota)."""
    engine = driver.Engine(engine_path)
    times = {op: None for op in OPERATIONS}
    note = ""
    try:
        engine.request(timeout, type="resize", width=1280, height=800)
        nav = engine.request(timeout, type="navigate", url=url)
        if nav.get("type") != "state":
            return times, f"{nav.get('type')}: {nav.get('message', '')}".strip(": ")
        times["cargar"] = nav["seconds"]
        if out_png and nav.get("screenshot"):
            import base64

            with open(out_png, "wb") as f:
                f.write(base64.b64decode(nav["screenshot"]))
        if count_js:
            note = f"js {count_js_ok(engine, timeout)}/{JS_CHECKS}"
        steps = (
            ("scroll", dict(type="scroll", dx=0, dy=600)),
            ("clic", dict(type="click", x=2, y=2)),
            ("resize", dict(type="resize", width=1080, height=700)),
        )
        for op, payload in steps:
            response = engine.request(timeout, **payload)
            if response.get("type") != "state":
                note = f"{op}: {response.get('type')}"
                times[op] = response.get("seconds") if response.get("type") == "TIMEOUT" else None
                break
            times[op] = response["seconds"]
    finally:
        engine.close()
    return times, note


def breaches(times, limits):
    return [op for op in OPERATIONS if times[op] is None or times[op] > limits[op]]


def cell(value, limit):
    if value is None:
        return "-"
    return f"{value:.2f}" + ("!" if value > limit else "")


def main():
    # El motor avisa por stderr de cosas que aqui estorban; solo errores.
    os.environ.setdefault("RUST_LOG", "error")
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--mirror", action="store_true", help="carga las paginas reales a traves de un espejo local")
    parser.add_argument("--strict", action="store_true", help="usa los objetivos de la Fase 1 como limites")
    parser.add_argument("--only", action="append", default=[], help="solo esta pagina (se puede repetir)")
    parser.add_argument("--out-dir", help="carpeta donde guardar la captura de cada pagina")
    parser.add_argument("--engine", default=os.environ.get("ENGINE", driver.DEFAULT_ENGINE))
    args = parser.parse_args()

    limits = GOAL_LIMIT if args.strict else HARD_LIMIT
    pages = [p for p in PAGES if not args.only or p[0] in args.only]
    fixtures = [f for f in FIXTURES if not args.only or f[0] in args.only]
    if not pages and not fixtures:
        parser.error("--only no coincide con ninguna pagina")
    if args.out_dir:
        os.makedirs(args.out_dir, exist_ok=True)

    jobs = []
    if fixtures:
        port = driver.serve_fixtures()
        jobs += [(name, f"http://127.0.0.1:{port}/{file}", name == "fixture-js") for name, file in fixtures]
    ports = start_mirrors(pages) if args.mirror and pages else {}
    for name, url in pages:
        jobs.append((name, through_mirror(url, ports) if args.mirror else url, False))

    rows, failed = [], False
    for name, url, count_js in jobs:
        out_png = os.path.join(args.out_dir, f"{name}.png") if args.out_dir else None
        times, note = measure(args.engine, url, OPERATION_TIMEOUT, out_png, count_js)
        bad = breaches(times, limits)
        failed = failed or bool(bad)
        rows.append((name, times, note, bad))

    header = f"{'pagina':<12}" + "".join(f"{op:>9}" for op in OPERATIONS) + "  resultado"
    print(f"limites ({'objetivo Fase 1' if args.strict else 'que nada se cuelgue'}): " + ", ".join(f"{op} <= {limits[op]:g}s" for op in OPERATIONS))
    print(header)
    print("-" * len(header) + "-----------")
    for name, times, note, bad in rows:
        line = f"{name:<12}" + "".join(f"{cell(times[op], limits[op]):>9}" for op in OPERATIONS)
        line += "  " + ("FALLA " + ",".join(bad) if bad else "ok")
        if note:
            line += f"  [{note}]"
        print(line)
    print("('!' = supera el limite, '-' = no se pudo medir)")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
