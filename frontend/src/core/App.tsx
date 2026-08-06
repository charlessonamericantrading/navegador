import { useState, useEffect, useRef } from 'react';
import BrowserViewport from '../domains/browser/components/BrowserViewport';
import WelcomeGuide from '../domains/onboarding/components/WelcomeGuide';
import './App.css';

interface ElementRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface InteractiveElement {
  id: number;
  tagName: string;
  text: string;
  rect: ElementRect;
  selector: string;
  attributes: {
    id?: string;
    name?: string;
    placeholder?: string;
    type?: string;
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
  const [loading, setLoading] = useState(false);

  // Interactividad UI
  const [showOnboarding, setShowOnboarding] = useState(
    () => localStorage.getItem('onboarding_completed') !== 'true'
  );
  const [toast, setToast] = useState<string | null>(null);

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

  const showToast = (message: string) => {
    setToast(message);
    window.clearTimeout(toastTimeoutRef.current);
    toastTimeoutRef.current = window.setTimeout(() => setToast(null), 6000);
  };

  const beginLoading = () => {
    setLoading(true);
    window.clearTimeout(loadingTimeoutRef.current);
    loadingTimeoutRef.current = window.setTimeout(() => {
      setLoading(false);
      showToast('La página está tardando demasiado en responder. Puedes intentarlo de nuevo.');
      // La respuesta puede no haber llegado porque la conexión en sí está
      // muerta sin que el navegador lo haya notado todavía: se fuerza un
      // ciclo de reconexión en vez de esperar a que WebSocket lo detecte.
      wsRef.current?.close();
    }, 12000);
  };

  const endLoading = () => {
    setLoading(false);
    window.clearTimeout(loadingTimeoutRef.current);
  };

  // Solo toca refs, nunca estado, así que puede llamarse desde el onmessage del
  // WebSocket (que capturó el primer render) sin quedarse obsoleta.
  const flushScroll = () => {
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
      flushScroll();
    }, 3000);
  };

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
  }, []);

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

  // Conexión WebSocket con reconexión automática
  useEffect(() => {
    let reconnectTimeout: number;
    // Propio de este montaje del efecto (no de wsRef, que puede pasar a apuntar
    // a un socket más nuevo): evita que el cierre disparado por la propia
    // limpieza de este efecto programe una reconexión huérfana que nadie podrá
    // cancelar ya (se nota sobre todo con el doble-montaje de StrictMode/HMR).
    let cancelled = false;

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
            setScreenshot(data.screenshot);
            setBrowserUrl(data.url);
            setElements(data.elements || []);
            endLoading();
            window.clearTimeout(scrollTimeoutRef.current);
            scrollBusyRef.current = false;
            flushScroll();
            break;

          case 'agent_status':
            if (data.status === 'thinking') {
              beginLoading();
            } else if (data.status === 'idle' || data.status === 'error' || data.status === 'finished') {
              // El backend puede llegar a estos estados (agente cancelado o con
              // error) sin enviar antes un browser_state/error_msg, que son los
              // otros dos únicos sitios donde loading se desactiva: sin esto el
              // spinner de la barra de direcciones se queda girando para siempre.
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
  }, []);

  // Eventos manuales del usuario en el navegador
  // Se ignora la acción si ya hay una en curso (loading===true): cada acción
  // tarda 1-2.5s en el backend, así que sin esta guarda se podían encolar
  // varias acciones que acababan ejecutándose sobre coordenadas de una página
  // que ya había cambiado respecto a la que el usuario veía al pulsar.
  const handleManualNavigate = (url: string) => {
    if (loading) return;
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      beginLoading();
      wsRef.current.send(JSON.stringify({ type: 'navigate', url }));
    }
  };

  const handleManualClick = (x: number, y: number) => {
    if (loading) return;
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      beginLoading();
      wsRef.current.send(JSON.stringify({ type: 'click', x, y }));
    }
  };

  const handleManualType = (x: number, y: number, text: string) => {
    if (loading) return;
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      beginLoading();
      wsRef.current.send(JSON.stringify({ type: 'type', x, y, text }));
    }
  };

  // No se bloquea por `loading`: redimensionar el viewport no es una acción
  // sobre la página (no hay resultado que pueda quedar encolado sobre un
  // estado obsoleto), y el usuario puede redimensionar la ventana mientras
  // el agente está trabajando.
  // Tampoco se bloquea por `loading`: en un navegador real puedes seguir
  // desplazándote mientras la página trabaja.
  const handleManualScroll = (dy: number) => {
    scrollPendingRef.current += dy;
    flushScroll();
  };

  const handleManualResize = (width: number, height: number) => {
    lastSizeRef.current = { width, height };
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify({ type: 'resize', width, height }));
    }
  };

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
          onManualNavigate={handleManualNavigate}
          onManualClick={handleManualClick}
          onManualType={handleManualType}
          onManualResize={handleManualResize}
          onManualScroll={handleManualScroll}
          loading={loading}
        />
      </main>
    </div>
  );
}

export default App;
