# -*- coding: utf-8 -*-
"""Cliente del motor Rust nativo, sin Chromium ni Playwright.

El proceso Rust habla NDJSON por stdin/stdout. La ruta del ejecutable se debe
proporcionar explícitamente mediante ``NATIVE_ENGINE_PATH``; no se descarga,
busca ni sustituye por otro motor. Mientras esa variable no apunte a un
binario válido, el backend permanece disponible y expone un estado vacío.
"""

import asyncio
import json
import os
import sys
from pathlib import Path
from typing import Any, Dict, Optional


class NativeEngineUnavailableError(RuntimeError):
    """El motor Rust todavía no está conectado al backend."""

    def __init__(self):
        super().__init__("motor de navegador no disponible")


class NativeEngineClient:
    """Transporte NDJSON mínimo para el proceso ``engine_server``."""

    protocol_version = 1

    def __init__(self, executable_path: Optional[str] = None):
        configured_path = executable_path or os.environ.get("NATIVE_ENGINE_PATH")
        if configured_path and Path(configured_path).is_file():
            self.executable_path = Path(configured_path)
        else:
            self.executable_path = self._discover_engine_path()
        self.process: Optional[asyncio.subprocess.Process] = None
        self._lock = asyncio.Lock()
        self._stderr_task: Optional[asyncio.Task] = None
        self.renderer_ready = False
        self._request_number = 0

    @staticmethod
    def _discover_engine_path() -> Optional[Path]:
        name = "engine_server.exe" if sys.platform == "win32" else "engine_server"
        if getattr(sys, "frozen", False):
            base_dir = Path(sys.executable).parent.parent
            packaged_path = base_dir / "engine" / name
            if packaged_path.is_file():
                return packaged_path
        
        # Buscar en los directorios padre hasta encontrar la carpeta 'engine'
        current = Path(__file__).resolve().parent
        while current != current.parent:
            for mode in ("release", "debug"):
                cand = current / "engine" / "target" / mode / name
                if cand.is_file():
                    return cand
            current = current.parent
        return None

    @property
    def connected(self) -> bool:
        return self.process is not None and self.process.returncode is None

    async def start(self, width: int, height: int) -> bool:
        if self.connected:
            return self.renderer_ready
        if self.executable_path is None or not self.executable_path.is_file():
            return False

        try:
            self.process = await asyncio.create_subprocess_exec(
                str(self.executable_path),
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                limit=64 * 1024 * 1024,
            )
            self._stderr_task = asyncio.create_task(self._drain_stderr())
            ready = await asyncio.wait_for(self._read_response(), timeout=10)
            if (
                ready.get("type") != "ready"
                or ready.get("protocol_version") != self.protocol_version
                or ready.get("renderer") != "rust-native"
            ):
                raise RuntimeError("handshake NDJSON inválido")

            response = await self.request({
                "type": "resize",
                "id": "backend-start",
                "width": width,
                "height": height,
            })
            if response.get("type") == "error":
                raise RuntimeError(response.get("message", "error de resize"))
            self.renderer_ready = response.get("renderer_status") == "ready"
            return self.renderer_ready
        except (OSError, asyncio.TimeoutError, RuntimeError, ValueError) as error:
            print(f"[NativeEngineClient] No se pudo conectar con Rust: {error}")
            await self.close()
            return False

    async def request(self, payload: Dict[str, Any]) -> Dict[str, Any]:
        if not self.connected or self.process is None or self.process.stdin is None:
            raise NativeEngineUnavailableError()

        async with self._lock:
            self._request_number += 1
            request_payload = dict(payload)
            request_payload["id"] = f"python-{self._request_number}"
            try:
                self.process.stdin.write(
                    (json.dumps(request_payload, separators=(",", ":")) + "\n").encode()
                )
                await self.process.stdin.drain()
                return await asyncio.wait_for(self._read_response(), timeout=30)
            except (BrokenPipeError, ConnectionError, asyncio.TimeoutError, RuntimeError, ValueError):
                await self.close()
                raise

    async def _read_response(self) -> Dict[str, Any]:
        if self.process is None or self.process.stdout is None:
            raise NativeEngineUnavailableError()
        line = await self.process.stdout.readline()
        if not line:
            raise RuntimeError("el proceso Rust cerró stdout")
        response = json.loads(line.decode("utf-8"))
        if not isinstance(response, dict):
            raise ValueError("la respuesta NDJSON no es un objeto")
        return response

    async def _drain_stderr(self):
        if self.process is None or self.process.stderr is None:
            return
        while await self.process.stderr.readline():
            pass

    async def close(self):
        process = self.process
        self.process = None
        self.renderer_ready = False
        if process is None:
            return

        if process.returncode is None and process.stdin is not None:
            try:
                process.stdin.write(b'{"type":"shutdown","id":"backend-close"}\n')
                await process.stdin.drain()
                await asyncio.wait_for(process.wait(), timeout=2)
            except Exception:
                try:
                    process.kill()
                    await process.wait()
                except Exception:
                    pass

        if self._stderr_task is not None:
            self._stderr_task.cancel()
            self._stderr_task = None


class BrowserManager:
    """Fachada del renderer Rust; nunca tiene fallback a un navegador externo."""

    def __init__(self, width: int = 1280, height: int = 720):
        self.width = width
        self.height = height
        self._client = NativeEngineClient()
        self._native_engine_connected = False
        self._notice_printed = False

    async def start(self):
        self._native_engine_connected = await self._client.start(self.width, self.height)
        if not self._native_engine_connected and not self._notice_printed:
            print("[BrowserManager] Motor nativo Rust no conectado todavía; no se usará Chromium.")
            self._notice_printed = True

    async def _require_native_engine(self):
        await self.start()
        if not self._native_engine_connected:
            raise NativeEngineUnavailableError()

    async def resize(self, width: int, height: int):
        self.width = width
        self.height = height
        await self.start()
        if self._native_engine_connected:
            response = await self._client.request({
                "type": "resize",
                "id": "backend-resize",
                "width": width,
                "height": height,
            })
            if response.get("type") == "error":
                raise RuntimeError(response.get("message", "error de resize"))

    async def navigate(self, url: str):
        await self._require_native_engine()
        response = await self._client.request({
            "type": "navigate",
            "id": "backend-navigate",
            "url": url,
        })
        if response.get("type") == "error":
            raise RuntimeError(response.get("message", "error de navegación"))

    async def click(self, x: int, y: int):
        await self._require_native_engine()
        response = await self._client.request({
            "type": "click",
            "id": "backend-click",
            "x": x,
            "y": y,
        })
        if response.get("type") == "error":
            raise RuntimeError(response.get("message", "error de clic"))

    async def scroll(self, dx: int, dy: int):
        await self._require_native_engine()
        response = await self._client.request({
            "type": "scroll",
            "id": "backend-scroll",
            "dx": dx,
            "dy": dy,
        })
        if response.get("type") == "error":
            raise RuntimeError(response.get("message", "error de scroll"))

    async def type_text(self, x: int, y: int, text: str, press_enter: bool = False):
        await self._require_native_engine()
        response = await self._client.request({
            "type": "type_text",
            "id": "backend-type",
            "x": x,
            "y": y,
            "text": text,
            "press_enter": press_enter,
        })
        if response.get("type") == "error":
            raise RuntimeError(response.get("message", "error de entrada de texto"))

    async def press_key(self, key: str):
        await self._require_native_engine()
        response = await self._client.request({
            "type": "press_key",
            "id": "backend-key",
            "key": key,
        })
        if response.get("type") == "error":
            raise RuntimeError(response.get("message", "error de teclado"))

    async def _get_state(self) -> Dict[str, Any]:
        await self.start()
        if not self._native_engine_connected:
            return {"screenshot": "", "url": "", "title": "", "elements": []}
        response = await self._client.request({"type": "get_state", "id": "backend-state"})
        if response.get("type") == "error":
            raise RuntimeError(response.get("message", "error leyendo estado"))
        return response

    async def get_state(self) -> Dict[str, Any]:
        """Obtiene una sola instantánea para la actualización del frontend."""
        return await self._get_state()

    async def get_screenshot(self) -> str:
        return str((await self._get_state()).get("screenshot", ""))

    async def get_elements(self):
        return (await self._get_state()).get("elements", [])

    async def get_url(self) -> str:
        return str((await self._get_state()).get("url", ""))

    async def get_title(self) -> str:
        return str((await self._get_state()).get("title", ""))

    async def close(self):
        await self._client.close()
        self._native_engine_connected = False
