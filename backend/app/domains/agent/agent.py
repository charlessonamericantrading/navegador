# -*- coding: utf-8 -*-
import json
import asyncio
from google import genai
from google.genai import types
from app.domains.browser.browser import BrowserManager
from app.domains.browser.parser import get_simplified_dom_text

def _friendly_gemini_error(exc: Exception) -> str:
    """
    Traduce los errores más comunes de la API de Gemini a mensajes que
    cualquier persona pueda entender, sin exponer detalles técnicos (códigos
    HTTP, trazas de la librería, etc.) en la interfaz.
    """
    text = str(exc).lower()

    if "api_key_invalid" in text or "api key not valid" in text or "permission_denied" in text:
        return "Tu clave de API de Gemini no es válida. Revísala en Ajustes o genera una nueva en Google AI Studio."
    if "resource_exhausted" in text or "429" in text or "quota" in text:
        return "Has alcanzado el límite de uso gratuito de Gemini por ahora. Espera unos minutos o revisa tu cuota en Google AI Studio."
    if "unavailable" in text or "deadline" in text or "timeout" in text:
        return "Gemini no respondió a tiempo. Puede ser un problema de conexión; inténtalo de nuevo."
    if "json" in text or "decode" in text:
        return "El agente recibió una respuesta inesperada de Gemini. Vuelve a intentarlo; si persiste, prueba con el modo gratuito."
    return "No se ha podido completar la consulta a Gemini. Vuelve a intentarlo en unos segundos."


class AgentOrchestrator:
    def __init__(self, browser: BrowserManager):
        self.browser = browser
        self.history = []  # Guarda la lista de pasos ejecutados: {"thought": "", "action": ""}

    def reset(self):
        self.history = []

    async def run_step(self, goal: str, mode: str, api_key: str = None) -> dict:
        """
        Ejecuta un único paso del bucle de agente.
        Devuelve un diccionario con el pensamiento, la acción ejecutada, la URL actual y si ha finalizado.
        """
        url = await self.browser.get_url()
        title = await self.browser.get_title()
        elements = await self.browser.get_elements()
        
        # 1. Generar la representación simplificada del DOM
        dom_text = get_simplified_dom_text(elements)
        
        # 2. Obtener la decisión del agente (Simulado o Gemini)
        if mode == "simulation":
            step_result = await self._run_simulated_step(goal, url, title, elements, dom_text)
        else:
            step_result = await self._run_gemini_step(goal, url, title, elements, dom_text, api_key)
            
        # 3. Ejecutar la acción en el navegador
        action_type = step_result.get("action")
        target_id = step_result.get("target_id")
        text = step_result.get("text", "")
        key = step_result.get("key", "")
        nav_url = step_result.get("url", "")
        
        execution_msg = ""
        finished = False
        answer = ""
        
        try:
            if action_type == "navigate":
                execution_msg = f"Navegando a {nav_url}"
                await self.browser.navigate(nav_url)
            elif action_type == "click":
                # Buscar las coordenadas del elemento
                el = next((e for e in elements if e["id"] == target_id), None)
                if el:
                    rect = el["rect"]
                    # Clicar en el centro del elemento
                    cx = rect["x"] + rect["width"] // 2
                    cy = rect["y"] + rect["height"] // 2
                    execution_msg = f"Haciendo clic en [{target_id}] {el['tagName']} '{el['text']}'"
                    await self.browser.click(cx, cy)
                else:
                    execution_msg = f"Error: Elemento [{target_id}] no encontrado"
            elif action_type == "type":
                el = next((e for e in elements if e["id"] == target_id), None)
                if el:
                    rect = el["rect"]
                    cx = rect["x"] + rect["width"] // 2
                    cy = rect["y"] + rect["height"] // 2
                    execution_msg = f"Escribiendo '{text}' en [{target_id}] {el['tagName']}"
                    await self.browser.type_text(cx, cy, text, press_enter=True)
                else:
                    execution_msg = f"Error: Campo de entrada [{target_id}] no encontrado"
            elif action_type == "press":
                execution_msg = f"Presionando tecla '{key}'"
                await self.browser.press_key(key)
            elif action_type == "finish":
                execution_msg = "Objetivo completado"
                finished = True
                answer = step_result.get("answer", "He terminado la tarea.")
            else:
                execution_msg = "Acción no reconocida o inactividad"
                finished = True
                
        except Exception as e:
            execution_msg = f"Error al ejecutar acción: {str(e)}"
            finished = True
            
        # Registrar en el historial
        step_data = {
            "thought": step_result.get("thought", ""),
            "action": execution_msg,
            "url": url,
            "title": title,
            "finished": finished,
            "answer": answer
        }
        self.history.append(step_data)
        return step_data

    async def _run_simulated_step(self, goal: str, url: str, title: str, elements: list, dom_text: str) -> dict:
        """
        Un agente simulado que analiza las metas comunes (búsqueda en Google, Wikipedia)
        y ejecuta reglas basadas en los elementos reales detectados.
        """
        await asyncio.sleep(1.0) # Simular tiempo de "pensamiento"
        
        goal_lower = goal.lower()
        step_count = len(self.history)
        
        # REGLA 1: Navegar a Google o Wikipedia si estamos en blanco o empezamos
        if step_count == 0:
            if "wikipedia" in goal_lower:
                return {
                    "thought": "Para buscar información en Wikipedia, primero debo navegar a wikipedia.org.",
                    "action": "navigate",
                    "url": "wikipedia.org"
                }
            else:
                return {
                    "thought": "Para cumplir con la tarea, primero navegaré a Google para buscar información.",
                    "action": "navigate",
                    "url": "google.com"
                }

        # REGLA 2: Si estamos en Google y queremos buscar
        if "google.com" in url:
            # Buscar campo de texto de búsqueda
            search_input = next((e for e in elements if e["tagName"] == "INPUT" and 
                                 (e["attributes"].get("name") == "q" or "search" in e["text"].lower() or "buscar" in e["text"].lower())), None)
            if search_input:
                # Escribir consulta
                query = goal
                if "busca" in goal_lower:
                    query = goal.split("busca", 1)[-1].strip()
                elif "search" in goal_lower:
                    query = goal.split("search", 1)[-1].strip()
                
                # Quitar conectores iniciales
                for word in ["en wikipedia", "en google", "sobre", "about"]:
                    query = query.replace(word, "").strip()
                
                return {
                    "thought": f"He localizado el cuadro de búsqueda de Google (ID {search_input['id']}). Procedo a escribir la consulta: '{query}'.",
                    "action": "type",
                    "target_id": search_input["id"],
                    "text": query
                }
            
            # Buscar el primer resultado de búsqueda (enlaces que no sean de Google y estén abajo de la búsqueda)
            # En Google, los resultados de búsqueda suelen ser enlaces que contienen encabezados o textos descriptivos
            first_result = None
            for e in elements:
                if e["tagName"] == "A" and e["attributes"].get("href") and "google.com" not in e["attributes"]["href"]:
                    # Ignorar enlaces a políticas, ayuda, etc.
                    href = e["attributes"]["href"]
                    if not any(x in href for x in ["accounts.google", "support.google", "policies.google", "maps.google", "preferences"]):
                        first_result = e
                        break
            
            if first_result:
                return {
                    "thought": f"Veo la página de resultados de Google. El primer enlace relevante es '{first_result['text']}' (ID {first_result['id']}). Procedo a clicarlo.",
                    "action": "click",
                    "target_id": first_result["id"]
                }

        # REGLA 3: Si estamos en Wikipedia
        if "wikipedia.org" in url:
            # Si estamos en la página principal, buscar el campo de búsqueda
            search_input = next((e for e in elements if e["tagName"] == "INPUT" and 
                                 ("search" in e["selector"] or "search" in (e["attributes"].get("name") or ""))), None)
            if search_input and "search" in url:
                # Si ya estamos en resultados o artículo, no buscamos de nuevo a menos que sea necesario
                pass
            elif search_input and not any(p in url for p in ["/wiki/", "wiki="]):
                query = goal
                # Extraer lo que buscamos
                if "wikipedia" in goal_lower:
                    query = goal.replace("wikipedia", "").replace("busca", "").replace("buscar", "").strip()
                
                return {
                    "thought": f"Estoy en Wikipedia. Localizo el cuadro de búsqueda (ID {search_input['id']}) y escribo '{query}'.",
                    "action": "type",
                    "target_id": search_input["id"],
                    "text": query
                }
            
            # Si ya estamos en un artículo de Wikipedia
            if "/wiki/" in url:
                # Extraer texto principal como respuesta
                paragraphs = [e["text"] for e in elements if e["tagName"] == "A" and len(e["text"]) > 30]
                summary = f"Estoy en el artículo de Wikipedia sobre {title}. El texto más relevante que he encontrado es: '{paragraphs[0] if paragraphs else 'Contenido de la página'}'."
                return {
                    "thought": f"He llegado al artículo de Wikipedia '{title}'. He extraído el contenido clave y he completado la tarea.",
                    "action": "finish",
                    "answer": summary
                }

        # Caída por defecto si no encaja en las reglas anteriores:
        # Clicar el primer enlace relevante o terminar
        if step_count > 3:
            return {
                "thought": f"Llevo {step_count} pasos en la página '{title}'. Considero que he recopilado suficiente información para finalizar.",
                "action": "finish",
                "answer": f"He navegado hasta '{title}' ({url}) y completé la observación para la petición: '{goal}'."
            }
        
        # Buscar algún enlace o botón y clicarlo
        revol = next((e for e in elements if e["tagName"] == "A" and len(e["text"]) > 10), None)
        if revol:
            return {
                "thought": f"Observo la página actual. Procedo a explorar haciendo clic en el enlace '{revol['text']}' (ID {revol['id']}).",
                "action": "click",
                "target_id": revol["id"]
            }

        return {
            "thought": "No encuentro más elementos interactivos. Termino la navegación.",
            "action": "finish",
            "answer": "No hay más elementos con los que interactuar."
        }

    async def _run_gemini_step(self, goal: str, url: str, title: str, elements: list, dom_text: str, api_key: str) -> dict:
        """
        Ejecuta un paso real consultando la API de Gemini (utilizando la SDK oficial google-genai).
        """
        if not api_key:
            return {
                "thought": "Error: Se requiere una clave de API de Gemini para ejecutar el modo real.",
                "action": "finish",
                "answer": "Por favor, introduce tu Gemini API Key en el panel de Ajustes."
            }
            
        client = genai.Client(api_key=api_key)
        
        # Construir historial para el prompt
        history_str = ""
        for i, step in enumerate(self.history):
            history_str += f"Paso {i+1}:\n- Pensamiento: {step['thought']}\n- Acción ejecutada: {step['action']}\n"
            
        system_instruction = """
        Eres un agente de navegación web de Inteligencia Artificial de última generación.
        Tu objetivo es ayudar al usuario a completar una tarea en el navegador.
        Se te proporcionará:
        1. El objetivo del usuario (Goal).
        2. La URL actual y el título de la página.
        3. El árbol DOM interactivo simplificado en formato: [ID] tag "Descripción" (atributos).
        4. El historial de pasos anteriores.
        
        Debes responder en formato JSON estructurado con la siguiente estructura:
        {
          "thought": "Explicación breve de lo que ves y por qué decides hacer la siguiente acción",
          "action": "navigate" | "click" | "type" | "press" | "finish",
          "target_id": 12, // Requerido sólo para 'click' y 'type'. Debe coincidir con un ID de la lista.
          "text": "texto a escribir", // Requerido sólo para 'type'
          "key": "Enter" | "Backspace" | "Tab", // Requerido sólo para 'press'
          "url": "https://...", // Requerido sólo para 'navigate'
          "answer": "Respuesta final para el usuario si decides usar la acción 'finish'"
        }
        
        Reglas importantes:
        - Selecciona únicamente IDs válidos que estén presentes en la lista de elementos interactivos proporcionada.
        - Si necesitas buscar algo, escribe en el cuadro de texto y asegúrate de enviar la acción 'type' con el texto deseado.
        - Sé eficiente. Intenta resolver la tarea en el menor número de pasos posibles.
        - Si ya has alcanzado la información requerida, usa la acción 'finish' y proporciona la respuesta en el campo 'answer'.
        """
        
        prompt = f"""
        OBJETIVO DEL USUARIO: {goal}
        
        ESTADO ACTUAL:
        - URL: {url}
        - Título de página: {title}
        
        ELEMENTOS INTERACTIVOS EN LA PÁGINA (DOM SIMPLIFICADO):
        {dom_text}
        
        HISTORIAL DE PASOS PREVIOS:
        {history_str if history_str else "Ninguno. Acabas de comenzar."}
        
        Por favor, decide tu siguiente paso y responde EXACTAMENTE en el formato JSON indicado.
        """
        
        try:
            # Llamamos a gemini-2.5-flash ya que es rápido, barato y excelente siguiendo instrucciones en JSON
            response = client.models.generate_content(
                model='gemini-2.5-flash',
                contents=prompt,
                config=types.GenerateContentConfig(
                    system_instruction=system_instruction,
                    response_mime_type="application/json"
                )
            )
            
            result = json.loads(response.text.strip())
            return result
        except Exception as e:
            print(f"Error consultando Gemini: {e}")
            friendly = _friendly_gemini_error(e)
            return {
                "thought": "No he podido obtener una respuesta válida de Gemini, así que detengo la tarea.",
                "action": "finish",
                "answer": friendly
            }
