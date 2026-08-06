# -*- coding: utf-8 -*-
import sys
from pathlib import Path
# Asegurar que el directorio raíz de la app está en el path antes de importar 'app'
sys.path.append(str(Path(__file__).parent.parent.parent))

import asyncio
if sys.platform == 'win32':
    asyncio.set_event_loop_policy(asyncio.WindowsProactorEventLoopPolicy())

import os
import json
import asyncio
import uvicorn
from fastapi import FastAPI, WebSocket, WebSocketDisconnect
from fastapi.middleware.cors import CORSMiddleware
from app.domains.browser.browser import BrowserManager
from app.domains.agent.agent import AgentOrchestrator

def friendly_error(exc: Exception) -> str:
    """
    Traduce una excepción técnica a un mensaje breve en lenguaje sencillo
    para mostrar en la interfaz. El detalle técnico completo se sigue
    imprimiendo en la consola (para depuración), pero nunca llega al usuario final.
    """
    text = str(exc).lower()
    name = type(exc).__name__

    if "timeout" in text or "timeout" in name.lower():
        return "La página tardó demasiado en responder. Puede que la web esté caída o tu conexión sea lenta."
    if "net::" in text or "err_name_not_resolved" in text or "err_internet_disconnected" in text:
        return "No se ha podido conectar a esa dirección. Comprueba tu conexión a internet o que la URL sea correcta."
    if "invalid url" in text or "err_invalid" in text:
        return "Esa dirección no parece válida. Revisa que esté bien escrita."
    if "motor de navegador no disponible" in text:
        return "El navegador no tiene ningún motor de renderizado conectado todavía."
    return "Ha ocurrido un problema inesperado al interactuar con la página. Puedes intentarlo de nuevo."


app = FastAPI(title="AI Agent Browser Backend")

# Habilitar CORS para conectar con el frontend de React
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],  # En producción especificar puertos concretos
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

# Gestor de navegador global para esta sesión
browser_manager = BrowserManager()

async def send_browser_state(websocket: WebSocket):
    """Obtiene la captura y elementos del navegador y los envía por el WebSocket."""
    state = await browser_manager.get_state()
    
    await websocket.send_json({
        "type": "browser_state",
        "screenshot": state.get("screenshot", ""),
        "url": state.get("url", ""),
        "elements": state.get("elements", [])
    })

@app.on_event("startup")
async def startup_event():
    await browser_manager.start()
    print("Backend listo (motor nativo Rust pendiente de conexión; no se usará Chromium).")

@app.on_event("shutdown")
async def shutdown_event():
    await browser_manager.close()
    print("Servidor detenido.")

@app.websocket("/ws")
async def websocket_endpoint(websocket: WebSocket):
    await websocket.accept()
    print("Cliente conectado por WebSocket.")
    
    orchestrator = AgentOrchestrator(browser_manager)
    agent_task = None
    
    # Enviar estado inicial al conectar
    try:
        await send_browser_state(websocket)
    except Exception as e:
        print(f"Error al enviar estado inicial: {e}")

    async def run_agent_loop(goal: str, mode: str, api_key: str):
        """Bucle asíncrono para ejecutar los pasos del agente."""
        try:
            orchestrator.reset()
            max_steps = 15
            step = 0
            
            await websocket.send_json({
                "type": "agent_status",
                "status": "running",
                "message": "Agente iniciado..."
            })
            
            while step < max_steps:
                step += 1
                # Enviar estado actual de pensamiento
                await websocket.send_json({
                    "type": "agent_status",
                    "status": "thinking",
                    "step": step
                })
                
                # Ejecutar paso
                step_result = await orchestrator.run_step(goal, mode, api_key)
                
                # Enviar resultado del paso
                await websocket.send_json({
                    "type": "agent_step",
                    "step": step_result
                })
                
                # Enviar actualización del navegador
                await send_browser_state(websocket)
                
                # Si el agente ha terminado
                if step_result.get("finished", False):
                    await websocket.send_json({
                        "type": "agent_status",
                        "status": "finished",
                        "message": step_result.get("answer", "Tarea completada.")
                    })
                    break
                    
                # Retraso configurable entre pasos
                await asyncio.sleep(1.0)
                
            else:
                await websocket.send_json({
                    "type": "agent_status",
                    "status": "finished",
                    "message": "Se alcanzó el número máximo de pasos sin finalizar."
                })
                
        except asyncio.CancelledError:
            print("Bucle del agente cancelado por el usuario.")
            await websocket.send_json({
                "type": "agent_status",
                "status": "idle",
                "message": "Ejecución cancelada por el usuario."
            })
        except Exception as e:
            print(f"Error en el bucle del agente: {e}")
            await websocket.send_json({
                "type": "agent_status",
                "status": "error",
                "message": friendly_error(e)
            })

    try:
        while True:
            data = await websocket.receive_text()
            message = json.loads(data)
            msg_type = message.get("type")
            
            # --- ACCIONES MANUALES DE USUARIO ---
            # Cada acción se protege individualmente: si falla una, el usuario recibe
            # un aviso amigable y la sesión sigue viva (antes, cualquier excepción aquí
            # tumbaba todo el bucle del WebSocket sin explicación).
            try:
                if msg_type == "navigate":
                    url = message.get("url")
                    await websocket.send_json({"type": "status_msg", "message": f"Navegando a {url}..."})
                    await browser_manager.navigate(url)
                    await send_browser_state(websocket)

                elif msg_type == "click":
                    x = int(message.get("x"))
                    y = int(message.get("y"))
                    await websocket.send_json({"type": "status_msg", "message": f"Clic manual en ({x}, {y})..."})
                    await browser_manager.click(x, y)
                    await send_browser_state(websocket)

                elif msg_type == "type":
                    x = int(message.get("x"))
                    y = int(message.get("y"))
                    text = message.get("text")
                    press_enter = bool(message.get("press_enter", True))
                    await websocket.send_json({"type": "status_msg", "message": "Escribiendo texto..."})
                    await browser_manager.type_text(x, y, text, press_enter)
                    await send_browser_state(websocket)

                elif msg_type == "press":
                    key = message.get("key")
                    await browser_manager.press_key(key)
                    await send_browser_state(websocket)

                elif msg_type == "scroll":
                    # Sin status_msg: el scroll se dispara muchas veces seguidas
                    # y llenaría la consola sin aportar nada al usuario.
                    dx = int(message.get("dx", 0))
                    dy = int(message.get("dy", 0))
                    await browser_manager.scroll(dx, dy)
                    await send_browser_state(websocket)

                elif msg_type == "refresh_browser":
                    await send_browser_state(websocket)

                elif msg_type == "resize":
                    # El frontend informa del tamaño real de su contenedor - sin
                    # motor de renderizado conectado, BrowserManager.resize() solo
                    # guarda las medidas, no hay ningún viewport que ajustar todavía.
                    width = max(200, min(int(message.get("width")), 4000))
                    height = max(200, min(int(message.get("height")), 4000))
                    await browser_manager.resize(width, height)
                    await send_browser_state(websocket)
            except Exception as action_error:
                print(f"Error ejecutando acción '{msg_type}': {action_error}")
                await websocket.send_json({
                    "type": "error_msg",
                    "message": friendly_error(action_error)
                })
                
            # --- ACCIONES DEL AGENTE ---
            if msg_type == "agent_start":
                goal = message.get("goal")
                mode = message.get("mode", "simulation")
                api_key = message.get("api_key", None)
                
                # Si ya hay una tarea ejecutándose, cancelarla
                if agent_task and not agent_task.done():
                    agent_task.cancel()
                    
                agent_task = asyncio.create_task(run_agent_loop(goal, mode, api_key))
                
            elif msg_type == "agent_stop":
                if agent_task and not agent_task.done():
                    agent_task.cancel()
                    await websocket.send_json({
                        "type": "agent_status",
                        "status": "idle",
                        "message": "Agente detenido."
                    })
                agent_task = None
                
    except WebSocketDisconnect:
        print("Cliente desconectado del WebSocket.")
        if agent_task and not agent_task.done():
            agent_task.cancel()
    except Exception as e:
        print(f"Error en el websocket: {e}")

if __name__ == "__main__":
    import sys
    from pathlib import Path
    sys.path.append(str(Path(__file__).parent.parent.parent))
    uvicorn.run(app, host="127.0.0.1", port=8000)
