import { useState, useEffect, useRef } from 'react';
import SettingsPanel from '../domains/settings/components/SettingsPanel';
import BrowserViewport from '../domains/browser/components/BrowserViewport';
import AgentConsole from '../domains/agent/components/AgentConsole';
import DOMViewer from '../domains/browser/components/DOMViewer';
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

interface AgentStep {
  thought: string;
  action: string;
  url: string;
  title: string;
  finished: boolean;
  answer?: string;
}

function App() {
  // Ajustes persistidos
  const [apiKey, setApiKeyState] = useState(() => localStorage.getItem('gemini_api_key') || '');
  const [agentMode, setAgentModeState] = useState(() => localStorage.getItem('agent_mode') || 'simulation');
  const [agentSpeed, setAgentSpeedState] = useState(() => Number(localStorage.getItem('agent_speed')) || 2);

  // Estados del navegador
  const [screenshot, setScreenshot] = useState('');
  const [browserUrl, setBrowserUrl] = useState('');
  const [elements, setElements] = useState<InteractiveElement[]>([]);
  const [loading, setLoading] = useState(false);

  // Estados del agente de IA
  const [agentStatus, setAgentStatus] = useState<'idle' | 'running' | 'thinking' | 'finished' | 'error'>('idle');
  const [agentMessage, setAgentMessage] = useState('');
  const [steps, setSteps] = useState<AgentStep[]>([]);

  // Interactividad UI
  const [hoveredElementId, setHoveredElementId] = useState<number | null>(null);
  const [activeTab, setActiveTab] = useState<'console' | 'dom'>('console');
  const [wsConnected, setWsConnected] = useState(false);
  const [showOnboarding, setShowOnboarding] = useState(
    () => localStorage.getItem('onboarding_completed') !== 'true'
  );
  const [pendingGoal, setPendingGoal] = useState<string | undefined>(undefined);
  const [toast, setToast] = useState<string | null>(null);

  const wsRef = useRef<WebSocket | null>(null);
  const toastTimeoutRef = useRef<number | undefined>(undefined);

  const showToast = (message: string) => {
    setToast(message);
    window.clearTimeout(toastTimeoutRef.current);
    toastTimeoutRef.current = window.setTimeout(() => setToast(null), 6000);
  };

  // Guardar configuraciones en localStorage al cambiar
  const setApiKey = (val: string) => {
    setApiKeyState(val);
    localStorage.setItem('gemini_api_key', val);
  };

  const setAgentMode = (val: string) => {
    setAgentModeState(val);
    localStorage.setItem('agent_mode', val);
  };

  const setAgentSpeed = (val: number) => {
    setAgentSpeedState(val);
    localStorage.setItem('agent_speed', val.toString());
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
        setWsConnected(true);
      };

      socket.onclose = () => {
        console.log('WebSocket cerrado. Intentando reconectar en 3s...');
        setWsConnected(false);
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
            // Los IDs de elemento se reasignan desde cero en cada captura del DOM
            // (ver backend/app/domains/browser/parser.py), así que un ID resaltado
            // de la instantánea anterior puede ahora apuntar a un elemento distinto.
            // Se limpia aquí para no resaltar algo que el usuario no está señalando.
            setHoveredElementId(null);
            setLoading(false);
            break;

          case 'agent_status':
            setAgentStatus(data.status);
            if (data.message) {
              setAgentMessage(data.message);
            }
            if (data.status === 'thinking') {
              setLoading(true);
            } else if (data.status === 'idle' || data.status === 'error' || data.status === 'finished') {
              // El backend puede llegar a estos estados (agente cancelado o con
              // error) sin enviar antes un browser_state/error_msg, que son los
              // otros dos únicos sitios donde loading se desactiva: sin esto el
              // spinner de la barra de direcciones se queda girando para siempre.
              setLoading(false);
            }
            break;

          case 'agent_step':
            setSteps(prev => [...prev, data.step]);
            break;

          case 'status_msg':
            console.log('Backend status:', data.message);
            break;

          case 'error_msg':
            setLoading(false);
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
      setLoading(true);
      wsRef.current.send(JSON.stringify({ type: 'navigate', url }));
    }
  };

  const handleManualClick = (x: number, y: number) => {
    if (loading) return;
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      setLoading(true);
      wsRef.current.send(JSON.stringify({ type: 'click', x, y }));
    }
  };

  const handleManualType = (x: number, y: number, text: string) => {
    if (loading) return;
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      setLoading(true);
      wsRef.current.send(JSON.stringify({ type: 'type', x, y, text }));
    }
  };

  // Eventos de control del Agente de IA
  const handleStartAgent = (goal: string) => {
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      setSteps([]);
      setAgentStatus('running');
      setAgentMessage('Iniciando agente...');
      wsRef.current.send(JSON.stringify({
        type: 'agent_start',
        goal,
        mode: agentMode,
        api_key: apiKey
      }));
    }
  };

  const handleStopAgent = () => {
    if (wsRef.current && wsRef.current.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify({ type: 'agent_stop' }));
    }
  };

  // Resalta un elemento al seleccionarlo en la lista lateral
  const handleElementSelect = (el: InteractiveElement) => {
    // Si el usuario clica el elemento en el árbol DOM, podemos simular un clic manual automáticamente
    const cx = el.rect.x + el.rect.width / 2;
    const cy = el.rect.y + el.rect.height / 2;
    handleManualClick(cx, cy);
  };

  // Genera un informe de diagnóstico y lo deja listo para que el propio usuario
  // decida enviarlo por correo. Nunca se transmite nada automáticamente ni sin
  // su intervención, y deliberadamente NUNCA se incluye la clave de API.
  const handleSendReport = () => {
    const lastSteps = steps.slice(-3).map((s, i) =>
      `  ${i + 1}. Pensamiento: ${s.thought || '(sin registrar)'} | Acción: ${s.action || '(sin registrar)'}`
    ).join('\n');

    const report = [
      `Fecha: ${new Date().toISOString()}`,
      `Plataforma: ${window.electronAPI?.platform || navigator.platform}`,
      `Modo del agente: ${agentMode}`,
      `Estado del agente: ${agentStatus}`,
      `Último mensaje: ${agentMessage || '(ninguno)'}`,
      `URL del navegador de pruebas: ${browserUrl || '(ninguna)'}`,
      `Últimos pasos:\n${lastSteps || '  (ninguno)'}`
    ].join('\n');

    const mailBody = encodeURIComponent(
      'He copiado los detalles técnicos al portapapeles. Pégalos aquí debajo:\n\n(pega aquí)\n\nDescribe brevemente qué esperabas que pasara:\n'
    );
    const mailtoUrl = `mailto:?subject=${encodeURIComponent('Informe de Navegador IA')}&body=${mailBody}`;

    navigator.clipboard.writeText(report).then(
      () => showToast('Informe copiado al portapapeles. Se abrirá tu programa de correo para que lo pegues y lo envíes si quieres.'),
      () => showToast('No se pudo copiar el informe automáticamente. Puedes describir el problema directamente en el correo.')
    );

    if (window.electronAPI?.openExternal) {
      window.electronAPI.openExternal(mailtoUrl);
    } else {
      window.location.href = mailtoUrl;
    }
  };

  return (
    <div className="app-container">
      {/* Cabecera */}
      <header className="app-header">
        <div className="logo-section">
          <div className="logo-icon">AI</div>
          <span className="logo-text">Navegador IA</span>
          <div className="native-engine-pill">
            <span className="native-engine-dot"></span>
            Motor: Chromium (Playwright)
          </div>
        </div>

        <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
          <button
            onClick={() => setShowOnboarding(true)}
            className="btn"
            title="Ver la guía de bienvenida otra vez"
            style={{
              background: 'rgba(255,255,255,0.04)',
              border: '1px solid var(--border-light)',
              color: 'var(--text-muted)',
              padding: '6px 12px',
              fontSize: '0.8rem'
            }}
          >
            ¿Cómo funciona?
          </button>

          <button
            onClick={handleSendReport}
            className="btn"
            title="Copia un informe técnico y abre tu correo para que lo envíes si quieres"
            style={{
              background: 'rgba(255,255,255,0.04)',
              border: '1px solid var(--border-light)',
              color: 'var(--text-muted)',
              padding: '6px 12px',
              fontSize: '0.8rem'
            }}
          >
            Reportar un problema
          </button>

          <div className="status-badge">
            <div className={`status-dot ${wsConnected ? (agentStatus === 'thinking' ? 'thinking' : 'active') : ''}`} />
            <span>
              {backendIssue?.status === 'failed'
                ? 'No se pudo iniciar el motor del agente'
                : backendIssue?.status === 'restarting'
                ? `Reiniciando el motor del agente (intento ${backendIssue.attempt})...`
                : wsConnected
                ? (agentStatus === 'thinking' ? 'Agente pensando' : 'Listo para usarse')
                : 'Preparando el motor del agente...'}
            </span>
          </div>
        </div>
      </header>

      {backendIssue?.status === 'failed' && (
        <div
          role="alert"
          style={{
            position: 'fixed',
            top: '64px',
            left: 0,
            right: 0,
            zIndex: 150,
            background: 'hsl(355, 40%, 16%)',
            borderBottom: '1px solid var(--error)',
            color: 'var(--text-main)',
            padding: '10px 24px',
            fontSize: '0.85rem',
            textAlign: 'center'
          }}
        >
          No se ha podido iniciar el motor del agente tras varios intentos. Cierra y vuelve a abrir
          la aplicación; si el problema continúa, reinstálala desde el instalador oficial.
        </div>
      )}

      {updateReady && (
        <div
          role="status"
          style={{
            position: 'fixed',
            top: '64px',
            left: 0,
            right: 0,
            zIndex: 150,
            background: 'hsl(150, 30%, 14%)',
            borderBottom: '1px solid var(--success)',
            color: 'var(--text-main)',
            padding: '10px 24px',
            fontSize: '0.85rem',
            textAlign: 'center',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            gap: '14px'
          }}
        >
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
          onFinish={(chosenGoal) => {
            localStorage.setItem('onboarding_completed', 'true');
            setShowOnboarding(false);
            if (chosenGoal) {
              setPendingGoal(chosenGoal);
            }
          }}
        />
      )}

      {toast && (
        <div
          role="alert"
          style={{
            position: 'fixed',
            bottom: '24px',
            left: '50%',
            transform: 'translateX(-50%)',
            zIndex: 200,
            background: 'hsl(355, 30%, 14%)',
            border: '1px solid var(--error)',
            color: 'var(--text-main)',
            borderRadius: '10px',
            padding: '12px 18px',
            display: 'flex',
            alignItems: 'center',
            gap: '12px',
            maxWidth: '480px',
            boxShadow: '0 8px 30px rgba(0, 0, 0, 0.4)',
            fontSize: '0.85rem'
          }}
        >
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

      {/* Sidebar Izquierdo: Configuración */}
      <SettingsPanel
        apiKey={apiKey}
        setApiKey={setApiKey}
        agentMode={agentMode}
        setAgentMode={setAgentMode}
        agentSpeed={agentSpeed}
        setAgentSpeed={setAgentSpeed}
      />

      {/* Área Principal: Viewport del Navegador */}
      <main className="main-viewport">
        <BrowserViewport
          screenshot={screenshot}
          url={browserUrl}
          elements={elements}
          onManualNavigate={handleManualNavigate}
          onManualClick={handleManualClick}
          onManualType={handleManualType}
          hoveredElementId={hoveredElementId}
          setHoveredElementId={setHoveredElementId}
          loading={loading}
        />
      </main>

      {/* Sidebar Derecho: Consola y DOM */}
      <aside className="right-panel">
        <div className="panel-tabs">
          <div
            className={`panel-tab ${activeTab === 'console' ? 'active' : ''}`}
            onClick={() => setActiveTab('console')}
          >
            Consola del Agente
          </div>
          <div
            className={`panel-tab ${activeTab === 'dom' ? 'active' : ''}`}
            onClick={() => setActiveTab('dom')}
          >
            Lo que ve el agente ({elements.length})
          </div>
        </div>

        <div className="panel-content">
          {activeTab === 'console' ? (
            <AgentConsole
              onStartAgent={handleStartAgent}
              onStopAgent={handleStopAgent}
              agentStatus={agentStatus}
              agentMessage={agentMessage}
              steps={steps}
              presetGoal={pendingGoal}
              onPresetConsumed={() => setPendingGoal(undefined)}
            />
          ) : (
            <DOMViewer
              elements={elements}
              hoveredElementId={hoveredElementId}
              setHoveredElementId={setHoveredElementId}
              onElementSelect={handleElementSelect}
            />
          )}
        </div>
      </aside>
    </div>
  );
}

export default App;
