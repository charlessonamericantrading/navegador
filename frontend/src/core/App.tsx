import { useState, useEffect, useLayoutEffect, useRef, useMemo, useCallback } from 'react';
import BrowserViewport from '../domains/browser/components/BrowserViewport';
import WelcomeGuide from '../domains/onboarding/components/WelcomeGuide';
import AgentSidebar from '../domains/agent/components/AgentSidebar';
import { BrowserActionError, type BrowserInterface } from '../domains/agent/AgentOrchestrator';
import type { EngineRequestPayload, EngineStateEvent } from '../electron';
import { NavigationSequence, isCancellationMessage, isNavigationCommand } from './navigationSequence';
import './App.css';

interface ElementRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface InteractiveElement {
  id: number;
  tag_name: string;
  text: string;
  rect: ElementRect;
  selector: string;
  attributes: {
    id?: string;
    name?: string;
    placeholder?: string;
    element_type?: string;
    role?: string;
    href?: string;
    value?: string;
  };
}

function App() {
  // Estados del navegador
  const [screenshot, setScreenshot] = useState('');
  const [browserUrl, setBrowserUrl] = useState('');
  const [elements, setElements] = useState<InteractiveElement[]>([]);
  // Fase 39: el motor avisa cuando la pagina se descargo bien pero su
  // contenido lo genera JavaScript que todavia no se ejecuta. Sin esto la
  // pantalla se quedaba en blanco sin un solo mensaje.
  const [requiresJavascript, setRequiresJavascript] = useState(false);
  // Desplazamiento vertical del documento. Las `rect` de `elements` son del
  // DOCUMENTO; para saber dónde están en la captura hay que restarlo.
  const [scrollOffsetY, setScrollOffsetY] = useState(0);
  const [loading, setLoading] = useState(false);
  // La última navegación que falló (tarea 1.3). Se pinta dentro del visor, no
  // solo como aviso flotante: antes la barra mostraba la dirección nueva sobre
  // el contenido de la página anterior, sin ninguna explicación.
  const [loadError, setLoadError] = useState<{ url: string; message: string } | null>(null);
  // Sube cada vez que hay que devolver la barra de direcciones a la página
  // que se está viendo (al detener una carga).
  const [addressResetKey, setAddressResetKey] = useState(0);
  // Numera las navegaciones: una respuesta atrasada no pisa la pantalla.
  const [navigation] = useState(() => new NavigationSequence());
  // Historial y pestañas: el motor los soporta desde hace tiempo y los
  // reporta en cada `state` (`can_go_back`/`can_go_forward`/`tab_id`),
  // pero la interfaz no los usaba. Se guardan aqui para poder
  // habilitar/deshabilitar los botones sin llevar una cuenta paralela.
  const [canGoBack, setCanGoBack] = useState(false);
  const [canGoForward, setCanGoForward] = useState(false);
  const [tabs, setTabs] = useState<{ id: number; title: string; url: string }[]>([]);
  const [activeTabId, setActiveTabId] = useState<number | null>(null);

  // Interactividad UI
  const [showOnboarding, setShowOnboarding] = useState(
    () => localStorage.getItem('onboarding_completed') !== 'true'
  );
  const [toast, setToast] = useState<string | null>(null);
  const [isAgentOpen, setIsAgentOpen] = useState(false);

  // Referencias en vivo para que el agente siempre lea el estado actualizado.
  // Se escriben en un efecto de layout (tras el commit, antes de cualquier
  // otro efecto o callback asincrono) y no durante el render, que React puede
  // repetir o descartar.
  const browserUrlRef = useRef(browserUrl);
  const elementsRef = useRef(elements);
  const tabsRef = useRef(tabs);
  const activeTabIdRef = useRef(activeTabId);
  useLayoutEffect(() => {
    browserUrlRef.current = browserUrl;
    elementsRef.current = elements;
    tabsRef.current = tabs;
    activeTabIdRef.current = activeTabId;
  });

  const wsRef = useRef<WebSocket | null>(null);
  const toastTimeoutRef = useRef<number | undefined>(undefined);
  // Último tamaño de contenedor conocido, informado por BrowserViewport. Se
  // guarda aquí (y no solo se envía) porque el contenedor puede medirse antes
  // de que el WebSocket llegue a abrirse; al abrir, se reenvía este valor para
  // que el primer screenshot ya llegue con el tamaño correcto.
  const lastSizeRef = useRef<{ width: number; height: number } | null>(null);
  // Estado del scroll. Cada rueda provoca una captura completa en el backend,
  // así que no se envía un mensaje por evento: mientras hay uno en vuelo se
  // acumula el desplazamiento y se manda de golpe al recibir la respuesta.
  const scrollBusyRef = useRef(false);
  const scrollPendingRef = useRef(0);
  const scrollTimeoutRef = useRef<number | undefined>(undefined);
  // Red de seguridad para clic/escritura/navegación: si por lo que sea la
  // respuesta nunca llega (mensaje perdido, backend reiniciado a medio
  // camino...), antes `loading` se quedaba en true para siempre y la app
  // dejaba de reaccionar a NINGÚN clic hasta reiniciarla entera.
  const loadingTimeoutRef = useRef<number | undefined>(undefined);

  // Aplica un mensaje `state` del motor. Antes esto estaba copiado en
  // tres sitios (IPC inicial, evento IPC, respuesta a comando) y cada uno
  // leia un subconjunto distinto de campos - de ahi que el historial y las
  // pestañas se perdieran por el camino.
  //
  // Este y los helpers siguientes van en `useCallback` sin dependencias
  // variables: solo tocan setters y refs, que React mantiene estables. Eso
  // permite declararlos en los efectos de conexion (lint exhaustivo) sin que
  // esos efectos se re-ejecuten y reconecten en cada render.
  const applyEngineState = useCallback((data: EngineStateEvent) => {
    setScreenshot(data.screenshot || '');
    setBrowserUrl(data.url || '');
    // `elements` y `requires_javascript` pueden no venir (la respuesta a un
    // `scroll` los omite: un desplazamiento no los cambia, y las coordenadas
    // de los elementos son del documento). Ausente = sin cambios; solo se
    // reemplazan cuando llegan.
    if (data.elements !== undefined) setElements(data.elements);
    if (data.requires_javascript !== undefined) setRequiresJavascript(Boolean(data.requires_javascript));
    if (typeof data.scroll_offset_y === 'number') setScrollOffsetY(data.scroll_offset_y);
    setCanGoBack(Boolean(data.can_go_back));
    setCanGoForward(Boolean(data.can_go_forward));
    if (typeof data.tab_id === 'number') setActiveTabId(data.tab_id);
  }, []);

  const showToast = useCallback((message: string) => {
    setToast(message);
    window.clearTimeout(toastTimeoutRef.current);
    toastTimeoutRef.current = window.setTimeout(() => setToast(null), 6000);
  }, []);

  const beginLoading = useCallback(() => {
    setLoading(true);
    window.clearTimeout(loadingTimeoutRef.current);
    loadingTimeoutRef.current = window.setTimeout(() => {
      if (window.electronAPI?.sendEngineRequest) {
        // La petición sigue viva: se avisa, pero `loading` se mantiene para
        // que el botón Detener siga ahí. Si el motor no contesta, el proceso
        // principal la da por fallida a los 30 s.
        showToast('La página está tardando en responder. Puedes esperar o pulsar Detener.');
        return;
      }
      setLoading(false);
      showToast('La página está tardando demasiado en responder. Puedes intentarlo de nuevo.');
      // La respuesta puede no haber llegado porque la conexión en sí está
      // muerta sin que el navegador lo haya notado todavía: se fuerza un
      // ciclo de reconexión en vez de esperar a que WebSocket lo detecte.
      wsRef.current?.close();
    }, 12000);
  }, [showToast]);

  const endLoading = useCallback(() => {
    setLoading(false);
    window.clearTimeout(loadingTimeoutRef.current);
  }, []);

  // Solo toca refs, nunca estado, así que puede llamarse desde el onmessage del
  // WebSocket (que capturó el primer render) sin quedarse obsoleta.
  // Funcion con nombre propio (`flush`) para poder reintentarse desde su
  // temporizador sin leer la constante antes de que exista.
  const flushScroll = useCallback(function flush() {
    if (scrollBusyRef.current) return;
    const dy = scrollPendingRef.current;
    if (!dy) return;
    if (!wsRef.current || wsRef.current.readyState !== WebSocket.OPEN) return;

    scrollPendingRef.current = 0;
    scrollBusyRef.current = true;
    wsRef.current.send(JSON.stringify({ type: 'scroll', dx: 0, dy }));

    // Red de seguridad: si el backend no llega a contestar (acción fallida,
    // reinicio del proceso), sin esto el scroll quedaría bloqueado para siempre.
    window.clearTimeout(scrollTimeoutRef.current);
    scrollTimeoutRef.current = window.setTimeout(() => {
      scrollBusyRef.current = false;
      flush();
    }, 3000);
  }, []);

  // `list_tabs` es la unica peticion que NO devuelve un `state`, sino su
  // propia respuesta con la lista - de ahi que se pida aparte y no salga
  // de `applyEngineState`. Se refresca tras cualquier accion que pueda
  // cambiar el conjunto de pestañas (abrir, cerrar, cambiar, y tambien
  // navegar, porque el titulo de la pestaña activa cambia con la pagina).
  const refreshTabs = useCallback(async () => {
    if (!window.electronAPI?.sendEngineRequest) return;
    try {
      const res = await window.electronAPI.sendEngineRequest({ type: 'list_tabs' });
      if (res?.type === 'tabs') {
        setTabs(res.tabs || []);
        if (typeof res.active_tab_id === 'number') setActiveTabId(res.active_tab_id);
      }
    } catch {
      // Sin pestañas que mostrar es un estado valido, no un error que
      // merezca molestar al usuario con un aviso.
    }
  }, []);

  // Peticiones que no muestran «cargando» (scroll, redimensionar): su `state`
  // se aplica salvo que una navegación o Detener lo haya dejado obsoleto.
  const sendEngineQuietly = useCallback(async (payload: EngineRequestPayload): Promise<void> => {
    if (!window.electronAPI?.sendEngineRequest) return;
    const ticket = navigation.ticket(payload.type);
    const res = await window.electronAPI.sendEngineRequest(payload);
    if (res?.type === 'state' && navigation.isCurrent(ticket)) applyEngineState(res);
  }, [applyEngineState, navigation]);

  // Escuchar reinicios/fallos del proceso backend (solo existe en la app de escritorio Electron)
  const [backendIssue, setBackendIssue] = useState<BackendStatusEvent | null>(null);
  useEffect(() => {
    if (!window.electronAPI?.onBackendStatus) {
      return;
    }
    const unsubscribe = window.electronAPI.onBackendStatus((data) => {
      setBackendIssue(data);
      if (data.status === 'restarting') {
        showToast('El motor del agente se detuvo. Reintentando iniciarlo automáticamente...');
      }
    });
    return unsubscribe;
  }, [showToast]);

  // Escuchar avisos de auto-actualización (solo existe en la app de escritorio Electron)
  const [updateReady, setUpdateReady] = useState(false);
  useEffect(() => {
    if (!window.electronAPI?.onUpdateStatus) {
      return;
    }
    const unsubscribe = window.electronAPI.onUpdateStatus((data) => {
      if (data.status === 'downloaded') {
        setUpdateReady(true);
      }
    });
    return unsubscribe;
  }, []);

  // Conexión unificada: IPC directo en Electron o WebSocket en navegador web
  useEffect(() => {
    let reconnectTimeout: number;
    let cancelled = false;

    // 1. MODO ELECTRON: Comunicación IPC directa con el motor Rust
    if (window.electronAPI?.sendEngineRequest && window.electronAPI?.onEngineState) {
      console.log('Modo Electron activo: usando transporte IPC nativo directo.');
      
      const unsubscribe = window.electronAPI.onEngineState((data) => {
        if (data.type === 'state') {
          // Solo lo espontáneo (`id` nulo: un temporizador de la página cambió
          // algo). Las respuestas con `id` las aplica quien hizo la petición,
          // que es quien sabe si siguen siendo vigentes (tarea 1.3).
          if (data.id == null) applyEngineState(data);
        } else if (data.type === 'ready') {
          console.log('Motor Rust listo vía IPC.');
          if (lastSizeRef.current) {
            sendEngineQuietly({ type: 'resize', ...lastSizeRef.current }).catch(console.error);
          }
        }
      });

      // Solicitar estado inicial o dimensionamiento
      const initialWidth = lastSizeRef.current?.width || 1280;
      const initialHeight = lastSizeRef.current?.height || 720;
      sendEngineQuietly({ type: 'resize', width: initialWidth, height: initialHeight })
        .then(() => refreshTabs())
        .catch((err) => console.log('Inicializando motor vía IPC...', err));

      return () => {
        unsubscribe();
      };
    }

    // 2. MODO NAVEGADOR WEB: Fallback mediante WebSocket a FastAPI
    const connectWS = () => {
      if (cancelled) return;
      console.log('Conectando al WebSocket del backend...');
      const socket = new WebSocket('ws://127.0.0.1:8000/ws');
      wsRef.current = socket;

      socket.onopen = () => {
        console.log('WebSocket conectado con éxito.');
        if (lastSizeRef.current) {
          socket.send(JSON.stringify({ type: 'resize', ...lastSizeRef.current }));
        }
      };

      socket.onclose = () => {
        console.log('WebSocket cerrado. Intentando reconectar en 3s...');
        if (!cancelled) {
          reconnectTimeout = window.setTimeout(connectWS, 3000);
        }
      };

      socket.onerror = (err) => {
        console.error('Error de WebSocket:', err);
        socket.close();
      };

      socket.onmessage = (event) => {
        let data;
        try {
          data = JSON.parse(event.data);
        } catch (err) {
          console.error('Mensaje WebSocket no es JSON válido:', event.data, err);
          return;
        }

        switch (data.type) {
          case 'browser_state':
            applyEngineState(data);
            endLoading();
            window.clearTimeout(scrollTimeoutRef.current);
            scrollBusyRef.current = false;
            flushScroll();
            break;

          case 'agent_status':
            if (data.status === 'thinking') {
              beginLoading();
            } else if (data.status === 'idle' || data.status === 'error' || data.status === 'finished') {
              endLoading();
            }
            break;

          case 'status_msg':
            console.log('Backend status:', data.message);
            break;

          case 'error_msg':
            endLoading();
            window.clearTimeout(scrollTimeoutRef.current);
            scrollBusyRef.current = false;
            scrollPendingRef.current = 0;
            showToast(data.message);
            break;

          default:
            break;
        }
      };
    };

    connectWS();

    return () => {
      cancelled = true;
      clearTimeout(reconnectTimeout);
      if (wsRef.current) {
        wsRef.current.close();
      }
    };
  }, [applyEngineState, beginLoading, endLoading, flushScroll, refreshTabs, sendEngineQuietly, showToast]);

  // Ejecuta un comando y FALLA si el motor no lo confirma (plan H15). Es la
  // unica ruta que usa el agente: un error del motor tiene que llegarle como
  // excepcion, no como un aviso en pantalla que el bucle del agente no ve.
  //
  // Por WebSocket (modo desarrollo con FastAPI) no hay respuesta
  // correlacionada, asi que el comando no se puede confirmar: se envia y se
  // falla de forma explicita en vez de dar por hecho que funciono.
  const runEngineCommand = useCallback(async (payload: EngineRequestPayload): Promise<void> => {
    if (window.electronAPI?.sendEngineRequest) {
      const navigates = isNavigationCommand(payload.type);
      if (navigates) setLoadError(null);
      const ticket = navigation.ticket(payload.type);
      beginLoading();
      try {
        const res = await window.electronAPI.sendEngineRequest(payload);
        // Una navegación posterior (o Detener) ya tomó el relevo: esta
        // respuesta describe una página que ya no interesa y no se aplica. El
        // `loading` lo terminará la petición vigente, no esta.
        if (!navigation.isCurrent(ticket)) return;
        if (res?.type === 'error') {
          if (navigates && !isCancellationMessage(res.message || '')) {
            const target = typeof payload.url === 'string' ? payload.url : browserUrlRef.current;
            setLoadError({ url: target, message: res.message || 'Error en acción del motor' });
          }
          throw new BrowserActionError(res.message || 'Error en acción del motor');
        }
        if (res?.type === 'state') {
          applyEngineState(res);
        }
        endLoading();
      } catch (err) {
        if (navigation.isCurrent(ticket)) endLoading();
        if (err instanceof BrowserActionError) throw err;
        throw new BrowserActionError(err instanceof Error ? err.message : 'Error comunicando con el motor nativo');
      }
      return;
    }
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      beginLoading();
      wsRef.current.send(JSON.stringify(payload));
      throw new BrowserActionError('El transporte WebSocket no confirma comandos: no se puede verificar la acción.');
    }
    throw new BrowserActionError('No hay conexión con el motor.');
  }, [applyEngineState, beginLoading, endLoading, navigation]);

  // Eventos manuales del usuario en el navegador: el mismo comando, con el
  // fallo convertido en aviso. Por WebSocket la falta de confirmacion es
  // normal aqui (la respuesta llega luego como `state`), asi que no se avisa.
  const sendCommand = async (payload: EngineRequestPayload): Promise<void> => {
    try {
      await runEngineCommand(payload);
    } catch (err) {
      if (!window.electronAPI?.sendEngineRequest && wsRef.current?.readyState === WebSocket.OPEN) return;
      // Una navegación fallida ya se muestra dentro del visor (`loadError`).
      if (isNavigationCommand(payload.type) && window.electronAPI?.sendEngineRequest) return;
      showToast(err instanceof Error ? err.message : String(err));
    }
  };

  const handleBack = () => {
    if (loading || !canGoBack) return;
    sendCommand({ type: 'back' });
  };

  const handleForward = () => {
    if (loading || !canGoForward) return;
    sendCommand({ type: 'forward' });
  };

  const handleNewTab = async () => {
    await sendCommand({ type: 'new_tab' });
    refreshTabs();
  };

  const handleSwitchTab = async (tabId: number) => {
    if (tabId === activeTabId) return;
    await sendCommand({ type: 'switch_tab', tab_id: tabId });
    refreshTabs();
  };

  const handleCloseTab = async (tabId: number) => {
    // El motor rechaza cerrar la ultima pestaña (mantiene la invariante de
    // que siempre hay al menos una); no se le pide siquiera, para no
    // mostrar un error por algo que es comportamiento normal.
    if (tabs.length <= 1) return;
    await sendCommand({ type: 'close_tab', tab_id: tabId });
    refreshTabs();
  };

  // Se puede pedir otra página con una carga en curso: la última petición
  // manda (el motor abandona la anterior y la respuesta vieja se ignora).
  const handleManualNavigate = async (url: string) => {
    await sendCommand({ type: 'navigate', url });
    refreshTabs();
  };

  // Detener la carga (tarea 1.3): lo que hubiera en vuelo queda obsoleto, el
  // motor abandona la navegación y la barra vuelve a la página que se ve.
  const handleStop = () => {
    navigation.invalidateAll();
    endLoading();
    setLoadError(null);
    setAddressResetKey((key) => key + 1);
    if (window.electronAPI?.sendEngineRequest) {
      window.electronAPI.sendEngineRequest({ type: 'stop' }).catch(console.error);
    }
  };

  const handleManualClick = (x: number, y: number) => {
    if (loading) return;
    sendCommand({ type: 'click', x, y });
  };

  // Por id de elemento: el motor sabe dónde está el scroll y desplaza el campo
  // a la vista (tarea 3.1). `replace`: la ventana emergente parte del valor
  // actual del campo, así que lo que se envía es el valor final.
  const handleManualType = (elementId: number, text: string) => {
    if (loading) return;
    sendCommand({ type: 'type_into_element', element_id: elementId, text, replace: true, press_enter: true });
  };

  const handleManualScroll = (dy: number) => {
    if (window.electronAPI?.sendEngineRequest) {
      sendEngineQuietly({ type: 'scroll', dx: 0, dy }).catch(console.error);
    } else {
      scrollPendingRef.current += dy;
      flushScroll();
    }
  };

  const handleManualResize = (width: number, height: number) => {
    lastSizeRef.current = { width, height };
    if (window.electronAPI?.sendEngineRequest) {
      sendEngineQuietly({ type: 'resize', width, height }).catch(console.error);
    } else if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify({ type: 'resize', width, height }));
    }
  };

  // Interfaz de navegación provista al orquestador del agente IA
  const browserInterface: BrowserInterface = useMemo(() => ({
    getUrl: async () => browserUrlRef.current,
    getTitle: async () => {
      const currentTab = tabsRef.current.find((t) => t.id === activeTabIdRef.current);
      return currentTab?.title || currentTab?.url || 'Página Web';
    },
    getElements: async () => elementsRef.current,
    // Todas por `runEngineCommand`: un fallo del motor tiene que llegar al
    // agente. `navigate` ya no pasa por `handleManualNavigate`, que volvia
    // sin hacer nada si la pagina estaba cargando.
    navigate: async (url: string) => {
      await runEngineCommand({ type: 'navigate', url });
      refreshTabs();
    },
    clickElement: async (elementId: number) => {
      await runEngineCommand({ type: 'click_element', element_id: elementId });
    },
    // Rellenar no envía (plan H17): enviar es `pressKey('Enter')`, un paso
    // aparte que el agente tiene que decidir. La escritura manual
    // (`handleManualType`) sigue enviando: la dispara el propio usuario al
    // confirmar la ventana emergente de texto del viewport.
    typeIntoElement: async (elementId: number, text: string) => {
      await runEngineCommand({ type: 'type_into_element', element_id: elementId, text, replace: true, press_enter: false });
    },
    pressKey: async (key: string) => {
      await runEngineCommand({ type: 'press_key', key });
    }
  }), [runEngineCommand, refreshTabs]);

  return (
    <div className="app-container">
      {backendIssue?.status === 'failed' && (
        <div role="alert" className="alert-banner error">
          No se ha podido iniciar el motor del agente tras varios intentos. Cierra y vuelve a abrir
          la aplicación; si el problema continúa, reinstálala desde el instalador oficial.
        </div>
      )}

      {updateReady && (
        <div role="status" className="alert-banner success">
          <span>✨ Hay una nueva versión lista para instalarse.</span>
          <button
            onClick={() => window.electronAPI?.installUpdate()}
            className="btn btn-primary"
            style={{ padding: '4px 12px', fontSize: '0.78rem' }}
          >
            Reiniciar y actualizar
          </button>
        </div>
      )}

      {showOnboarding && (
        <WelcomeGuide
          onFinish={() => {
            localStorage.setItem('onboarding_completed', 'true');
            setShowOnboarding(false);
          }}
        />
      )}

      {toast && (
        <div role="alert" className="toast">
          <span aria-hidden="true">⚠️</span>
          <span style={{ lineHeight: 1.4 }}>{toast}</span>
          <button
            onClick={() => setToast(null)}
            className="btn"
            aria-label="Cerrar aviso"
            style={{ background: 'transparent', color: 'var(--text-dim)', padding: '2px 6px', fontSize: '0.9rem' }}
          >
            ✕
          </button>
        </div>
      )}

      {/* Área Principal: Viewport del Navegador */}
      <main className="main-viewport">
        <BrowserViewport
          screenshot={screenshot}
          url={browserUrl}
          elements={elements}
          scrollOffsetY={scrollOffsetY}
          requiresJavascript={requiresJavascript}
          onManualNavigate={handleManualNavigate}
          onManualClick={handleManualClick}
          onManualType={handleManualType}
          onManualResize={handleManualResize}
          onManualScroll={handleManualScroll}
          loading={loading}
          loadError={loadError}
          addressResetKey={addressResetKey}
          onStop={handleStop}
          onRetry={() => loadError && handleManualNavigate(loadError.url)}
          canGoBack={canGoBack}
          canGoForward={canGoForward}
          onBack={handleBack}
          onForward={handleForward}
          tabs={tabs}
          activeTabId={activeTabId}
          onNewTab={handleNewTab}
          onSwitchTab={handleSwitchTab}
          onCloseTab={handleCloseTab}
          onToggleAgent={() => setIsAgentOpen((prev) => !prev)}
          isAgentOpen={isAgentOpen}
        />
      </main>

      {/* Panel Lateral: Agente Autónomo de IA */}
      <AgentSidebar
        isOpen={isAgentOpen}
        onClose={() => setIsAgentOpen(false)}
        browserInterface={browserInterface}
      />
    </div>
  );
};

export default App;
