# -*- coding: utf-8 -*-
import asyncio
import base64
from playwright.async_api import async_playwright
from app.domains.browser.parser import DOM_PARSER_JS

class BrowserManager:
    def __init__(self, width=1280, height=720):
        self.width = width
        self.height = height
        self.playwright = None
        self.browser = None
        self.context = None
        self.page = None

    async def start(self):
        """Inicia el navegador Chromium de Playwright."""
        if not self.playwright:
            self.playwright = await async_playwright().start()
            
        # Lanzamos de forma headless por defecto. 
        # Añadimos argumentos para evitar detección básica de automatización.
        self.browser = await self.playwright.chromium.launch(
            headless=True,
            args=[
                "--disable-blink-features=AutomationControlled",
                "--no-sandbox",
                "--disable-dev-shm-usage"
            ]
        )
        
        self.context = await self.browser.new_context(
            viewport={"width": self.width, "height": self.height},
            user_agent="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36",
            device_scale_factor=1
        )
        
        self.page = await self.context.new_page()
        # Inyectar script para ocultar el objeto navigator.webdriver
        await self.page.add_init_script(
            "const newProto = navigator.__proto__; delete newProto.webdriver; navigator.__proto__ = newProto;"
        )
        
        # Abrir página inicial en blanco o por defecto (Google)
        await self.page.goto("https://www.google.com")
        await asyncio.sleep(1)

    async def navigate(self, url: str):
        """Navega a la URL especificada."""
        if not self.page:
            await self.start()
            
        if not (url.startswith("http://") or url.startswith("https://")):
            url = "https://" + url
            
        try:
            # Esperar a que se dispare domcontentloaded
            await self.page.goto(url, wait_until="domcontentloaded", timeout=30000)
            await asyncio.sleep(1.5)  # Tiempo adicional para que el JS renderice la página
        except Exception as e:
            print(f"Error navegando a {url}: {e}")
            raise e

    async def click(self, x: int, y: int):
        """Realiza un clic físico en las coordenadas del viewport."""
        if not self.page:
            return
        # Simula mover el ratón y hacer clic de forma natural
        await self.page.mouse.move(x, y)
        await asyncio.sleep(0.1)
        await self.page.mouse.click(x, y)
        # Esperar un breve instante para ver cambios de UI inmediatos
        await asyncio.sleep(1.0)

    async def type_text(self, x: int, y: int, text: str, press_enter: bool = False):
        """Hace clic en unas coordenadas para enfocar y escribe texto."""
        if not self.page:
            return
        # Clic para enfocar el elemento
        await self.click(x, y)
        # Seleccionar todo el texto existente y borrarlo
        await self.page.keyboard.press("Control+A")
        await asyncio.sleep(0.05)
        await self.page.keyboard.press("Backspace")
        await asyncio.sleep(0.05)
        # Escribir el nuevo texto letra a letra (simulando retraso humano)
        for char in text:
            await self.page.keyboard.type(char)
            await asyncio.sleep(0.02)
            
        if press_enter:
            await asyncio.sleep(0.1)
            await self.page.keyboard.press("Enter")
            
        await asyncio.sleep(1.5)  # Esperar que reaccione la página

    async def press_key(self, key: str):
        """Presiona una tecla en el navegador (ej: Enter, Escape, Backspace)."""
        if not self.page:
            return
        await self.page.keyboard.press(key)
        await asyncio.sleep(1.0)

    async def get_screenshot(self) -> str:
        """Toma un screenshot JPEG en base64 para envío rápido."""
        if not self.page:
            return ""
        try:
            screenshot_bytes = await self.page.screenshot(type="jpeg", quality=80)
            return base64.b64encode(screenshot_bytes).decode('utf-8')
        except Exception as e:
            print(f"Error tomando captura: {e}")
            return ""

    async def get_elements(self):
        """Extrae la lista de elementos interactivos de la página activa."""
        if not self.page:
            return []
        try:
            elements = await self.page.evaluate(DOM_PARSER_JS)
            return elements
        except Exception as e:
            print(f"Error analizando DOM: {e}")
            return []

    async def get_url(self) -> str:
        """Devuelve la URL actual."""
        if self.page:
            return self.page.url
        return ""

    async def get_title(self) -> str:
        """Devuelve el título de la página actual."""
        if self.page:
            return await self.page.title()
        return ""

    async def close(self):
        """Cierra todos los recursos del navegador."""
        if self.page:
            await self.page.close()
        if self.context:
            await self.context.close()
        if self.browser:
            await self.browser.close()
        if self.playwright:
            await self.playwright.stop()
        self.page = None
        self.context = None
        self.browser = None
        self.playwright = None
