# -*- coding: utf-8 -*-
import asyncio
import base64
from app.domains.browser.browser import BrowserManager

async def test_main():
    print("Iniciando prueba de Playwright y Extractor DOM...")
    browser = BrowserManager()
    
    try:
        await browser.start()
        print(f"Navegador abierto. URL inicial: {await browser.get_url()}")
        
        # Probar navegación a Wikipedia
        print("Navegando a wikipedia.org...")
        await browser.navigate("wikipedia.org")
        
        url = await browser.get_url()
        title = await browser.get_title()
        print(f"URL actual: {url}")
        print(f"Título de página: {title}")
        
        # Probar captura de pantalla
        print("Tomando captura de pantalla...")
        screenshot_base64 = await browser.get_screenshot()
        if screenshot_base64:
            print("Captura tomada correctamente. Guardando en 'test_screenshot.jpg'...")
            with open("test_screenshot.jpg", "wb") as fh:
                fh.write(base64.b64decode(screenshot_base64))
            print("Archivo 'test_screenshot.jpg' creado con éxito.")
        else:
            print("Error: No se pudo generar la captura de pantalla.")
            
        # Probar extracción de elementos interactivos
        print("Analizando elementos interactivos del DOM...")
        elements = await browser.get_elements()
        print(f"Se detectaron {len(elements)} elementos interactivos.")
        
        # Mostrar los primeros 5
        print("\nPrimeros 5 elementos detectados:")
        for el in elements[:5]:
            clean_text = el['text'].encode('utf-8', errors='ignore').decode('utf-8', errors='ignore')
            try:
                print(f"- [{el['id']}] {el['tagName']}: '{clean_text}' en {el['rect']}")
            except UnicodeEncodeError:
                ascii_text = el['text'].encode('ascii', errors='ignore').decode('ascii')
                print(f"- [{el['id']}] {el['tagName']}: '{ascii_text}' en {el['rect']}")
            
    except Exception as e:
        print(f"Error durante la prueba: {e}")
    finally:
        print("Cerrando recursos del navegador...")
        await browser.close()
        print("Prueba completada.")

if __name__ == "__main__":
    asyncio.run(test_main())
