import React, { useState, useRef, useEffect } from 'react';
import { isTextEntryElement } from '../textEntry';

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

interface BrowserViewportProps {
  screenshot: string;
  url: string;
  elements: InteractiveElement[];
  /// Cuánto se ha desplazado el documento. `elements[].rect` es del
  /// documento y los clics del usuario son del viewport (la captura): sin
  /// restarlo, tras un scroll se buscaba el campo en el sitio equivocado.
  scrollOffsetY: number;
  /// Fase 39: el motor descargo la pagina sin error, pero venia sin
  /// contenido visible y con `<script>` que lo habrian generado. Se pinta
  /// un aviso encima en vez de dejar el area en blanco sin explicacion.
  requiresJavascript: boolean;
  onManualNavigate: (url: string) => void;
  /// `isTextField`: el clic cayó sobre un campo de texto, que desde ese momento
  /// recibe el teclado (el motor lo enfoca y le dibuja anillo y cursor).
  onManualClick: (x: number, y: number, isTextField: boolean) => void;
  /// Una tecla pulsada con el visor enfocado y un campo con el teclado.
  onManualKey: (key: string) => void;
  /// Hay un campo de texto con el teclado: las teclas van al motor.
  typingFocus: boolean;
  onManualResize: (width: number, height: number) => void;
  onManualScroll: (dy: number) => void;
  loading: boolean;
  /// La última navegación que falló: se pinta como una página de error dentro
  /// del visor, con la dirección que se pidió (tarea 1.3).
  loadError: { url: string; message: string } | null;
  /// Sube cuando hay que devolver la barra de direcciones a `url` (al detener
  /// una carga: la barra no debe quedarse con la página que ya no se carga).
  addressResetKey: number;
  onStop: () => void;
  onRetry: () => void;
  /// Historial y pestañas (el motor ya los soportaba; la interfaz no los
  /// exponia). `canGoBack`/`canGoForward` vienen del propio motor en cada
  /// `state`, asi que los botones reflejan el historial REAL en vez de una
  /// cuenta paralela que podria desincronizarse.
  canGoBack: boolean;
  canGoForward: boolean;
  onBack: () => void;
  onForward: () => void;
  tabs: { id: number; title: string; url: string }[];
  activeTabId: number | null;
  onNewTab: () => void;
  onSwitchTab: (tabId: number) => void;
  onCloseTab: (tabId: number) => void;
  onToggleAgent?: () => void;
  isAgentOpen?: boolean;
}

/**
 * Lo que el motor dice (`network_error: ...`, `http_error: 404 ...`) traducido
 * a una frase para quien no sabe qué es un error de red. El texto original se
 * muestra debajo, para quien lo necesite.
 */
function describeLoadError(message: string): string {
  if (message.startsWith('http_error')) return 'El servidor ha respondido con un error.';
  if (message.startsWith('network_error')) return 'No se ha podido conectar con el sitio. Comprueba la dirección y tu conexión.';
  if (message.startsWith('invalid_url')) return 'La dirección no es válida.';
  if (message.startsWith('esquema_no_soportado')) return 'Solo se pueden abrir direcciones http y https.';
  return 'La página no se ha podido abrir.';
}

export const BrowserViewport: React.FC<BrowserViewportProps> = ({
  screenshot,
  url,
  elements,
  scrollOffsetY,
  requiresJavascript,
  onManualNavigate,
  onManualClick,
  onManualKey,
  typingFocus,
  onManualResize,
  onManualScroll,
  loading,
  loadError,
  addressResetKey,
  onStop,
  onRetry,
  canGoBack,
  canGoForward,
  onBack,
  onForward,
  tabs,
  activeTabId,
  onNewTab,
  onSwitchTab,
  onCloseTab,
  onToggleAgent,
  isAgentOpen
}) => {
  const [addressInput, setAddressInput] = useState(url || 'https://www.google.com');
  const containerRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);

  // Informa al backend del tamaño real del contenedor (ver BrowserManager.resize
  // en el backend) - pensado para que, cuando haya un motor de renderizado
  // conectado, la captura llene el espacio disponible en vez de quedar con
  // proporción fija 1280:720 y franjas vacías a los lados.
  const lastSentSizeRef = useRef<{ width: number; height: number } | null>(null);
  const resizeDebounceRef = useRef<number | undefined>(undefined);

  useEffect(() => {
    if (!containerRef.current) return;

    const measureAndSend = () => {
      if (!containerRef.current) return;
      const { width, height } = containerRef.current.getBoundingClientRect();
      const w = Math.round(width);
      const h = Math.round(height);
      if (w <= 0 || h <= 0) return;
      if (lastSentSizeRef.current?.width === w && lastSentSizeRef.current?.height === h) return;
      lastSentSizeRef.current = { width: w, height: h };
      onManualResize(w, h);
    };

    const resizeObserver = new ResizeObserver(() => {
      // La primera medición (montaje) se envía sin demora para minimizar el
      // parpadeo inicial a 1280x720; los cambios posteriores (arrastrar el
      // borde de la ventana) se debouncean para no saturar al backend, que
      // en cada resize relanza un screenshot completo.
      if (lastSentSizeRef.current === null) {
        measureAndSend();
        return;
      }
      window.clearTimeout(resizeDebounceRef.current);
      resizeDebounceRef.current = window.setTimeout(measureAndSend, 250);
    });
    resizeObserver.observe(containerRef.current);

    return () => {
      resizeObserver.disconnect();
      window.clearTimeout(resizeDebounceRef.current);
    };
  }, [onManualResize]);

  // Cuando el motor navega (la URL cambia desde fuera), la barra muestra la
  // URL nueva. Se ajusta durante el render comparando con la URL anterior,
  // no en un efecto: el efecto pintaba primero la URL vieja y luego otra vez.
  const [previousUrl, setPreviousUrl] = useState(url);
  if (url !== previousUrl) {
    setPreviousUrl(url);
    if (url) setAddressInput(url);
  }

  // Detener una carga: la barra vuelve a la página que se está viendo. Mismo
  // patrón que arriba (se ajusta durante el render, no en un efecto).
  const [previousResetKey, setPreviousResetKey] = useState(addressResetKey);
  if (addressResetKey !== previousResetKey) {
    setPreviousResetKey(addressResetKey);
    setAddressInput(url || '');
  }

  // La barra de direcciones acepta tanto URLs ("wikipedia.org") como
  // busquedas en lenguaje natural ("clima en madrid"), igual que un
  // navegador real: solo lo primero es una direccion navegable, lo
  // segundo necesita pasar por un motor de busqueda o "navigate" fallaria
  // literalmente contra "https://clima en madrid".
  const looksLikeUrl = (input: string) => {
    if (/\s/.test(input)) return false;
    if (input.startsWith('http://') || input.startsWith('https://')) return true;
    if (input === 'localhost' || input.startsWith('localhost:')) return true;
    // Dominio con al menos un punto (ej. "wikipedia.org", "192.168.1.1") o
    // puerto explicito (ej. "127.0.0.1:8000").
    return /^[a-z0-9.-]+\.[a-z]{2,}(:\d+)?(\/.*)?$/i.test(input) || /^\d{1,3}(\.\d{1,3}){3}(:\d+)?$/.test(input);
  };

  const resolveAddressTarget = (raw: string) => {
    const input = raw.trim();
    if (looksLikeUrl(input)) {
      return input.startsWith('http://') || input.startsWith('https://') ? input : `https://${input}`;
    }
    return `https://www.google.com/search?q=${encodeURIComponent(input)}`;
  };

  const handleNavigateSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    // Se puede pedir otra página con una carga en curso: sustituye a la anterior.
    if (addressInput.trim()) {
      onManualNavigate(resolveAddressTarget(addressInput));
    }
  };

  const handleImageClick = (e: React.MouseEvent<HTMLImageElement>) => {
    // Evita encolar clics contra una página que ya ha cambiado: cada acción
    // tarda 1-2.5s en el backend (ver browser.py), así que sin esta guarda se
    // podían apilar varios clics que acababan ejecutándose sobre coordenadas
    // de una página distinta a la que el usuario veía al pulsar.
    if (loading) return;

    const rect = e.currentTarget.getBoundingClientRect();
    const clickX = e.clientX - rect.left;
    const clickY = e.clientY - rect.top;

    const naturalWidth = e.currentTarget.naturalWidth || rect.width;
    const naturalHeight = e.currentTarget.naturalHeight || rect.height;
    const scaleX = naturalWidth / rect.width;
    const scaleY = naturalHeight / rect.height;
    const mappedX = Math.round(clickX * scaleX);
    const mappedY = Math.round(clickY * scaleY);

    // `elements` está en coordenadas del documento; el clic, en las de la captura.
    const documentY = mappedY + scrollOffsetY;
    const isTextField = elements.some(el => {
      const { x, y, width, height } = el.rect;
      const isInside = mappedX >= x && mappedX <= x + width && documentY >= y && documentY <= y + height;
      return isInside && isTextEntryElement(el);
    });

    onManualClick(mappedX, mappedY, isTextField);
    // El visor recibe el teclado: sin esto el foco se quedaría en la barra de
    // direcciones y las teclas no llegarían aquí.
    containerRef.current?.focus();
  };

  // Teclado sobre el visor (tarea 3.2): con un campo de texto enfocado, cada
  // tecla va al motor, que escribe, borra o envía el formulario. Las
  // combinaciones con Ctrl/Alt/Meta se dejan al sistema (copiar, recargar...).
  const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (!typingFocus || e.ctrlKey || e.altKey || e.metaKey) return;
    if (e.key === 'Shift' || e.key === 'CapsLock' || e.key === 'Dead' || e.key === 'Process' || e.key === 'Unidentified') return;
    // Tab y F1-F12 siguen siendo del sistema.
    if (e.key === 'Tab' || /^F\d+$/.test(e.key)) return;
    // Espacio y Retroceso harían scroll o navegarían por su cuenta.
    e.preventDefault();
    onManualKey(e.key);
  };

  const handleWheel = (e: React.WheelEvent) => {
    // deltaMode indica la unidad de deltaY: 0 = píxeles, 1 = líneas, 2 = páginas.
    // El backend siempre espera píxeles.
    let dy = e.deltaY;
    if (e.deltaMode === 1) {
      dy *= 16;
    } else if (e.deltaMode === 2) {
      dy *= containerRef.current?.clientHeight ?? 800;
    }

    onManualScroll(Math.round(dy));
  };

  const quickLinks = [
    { name: 'Google', url: 'https://www.google.com', icon: '🔍' },
    { name: 'Wikipedia', url: 'https://es.wikipedia.org', icon: '📚' },
    { name: 'GitHub', url: 'https://github.com', icon: '💻' },
    { name: 'Noticias', url: 'https://news.google.com', icon: '📰' },
  ];

  return (
    <div className="browser-frame">
      {/* Barra de Navegación del Navegador (Nivel Superior Prominente) */}
      {/* Barra de pestañas - solo si hay mas de una, para no gastar
          espacio vertical cuando no aporta nada. */}
      {tabs.length > 1 && (
        <div className="tab-bar" role="tablist">
          {tabs.map((tab) => (
            <div
              key={tab.id}
              role="tab"
              aria-selected={tab.id === activeTabId}
              className={`tab${tab.id === activeTabId ? ' tab-active' : ''}`}
              onClick={() => onSwitchTab(tab.id)}
              title={tab.url || 'Pestaña nueva'}
            >
              <span className="tab-title">{tab.title || tab.url || 'Pestaña nueva'}</span>
              <button
                className="tab-close"
                aria-label={`Cerrar ${tab.title || 'pestaña'}`}
                onClick={(e) => {
                  // Sin esto, cerrar tambien cambiaria a esa pestaña
                  // (el clic llegaria al contenedor de arriba).
                  e.stopPropagation();
                  onCloseTab(tab.id);
                }}
              >
                ✕
              </button>
            </div>
          ))}
          <button className="tab-new" onClick={onNewTab} aria-label="Pestaña nueva" title="Pestaña nueva">
            +
          </button>
        </div>
      )}

      <div className="browser-bar">
        {/* Botones de control de navegación */}
        <div style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
          <button
            onClick={onBack}
            disabled={!canGoBack}
            className="btn icon-btn"
            title="Atrás"
            aria-label="Atrás"
          >
            ←
          </button>
          <button
            onClick={onForward}
            disabled={!canGoForward}
            className="btn icon-btn"
            title="Adelante"
            aria-label="Adelante"
          >
            →
          </button>
          {loading ? (
            <button
              onClick={onStop}
              className="btn icon-btn stop-btn"
              title="Detener la carga"
              aria-label="Detener la carga"
            >
              ✕
            </button>
          ) : (
            <button
              onClick={() => onManualNavigate(url || 'https://www.google.com')}
              className="btn icon-btn"
              title="Recargar página"
              aria-label="Recargar página"
            >
              ↻
            </button>
          )}
          {tabs.length <= 1 && (
            <button onClick={onNewTab} className="btn icon-btn" title="Pestaña nueva" aria-label="Pestaña nueva">
              +
            </button>
          )}
        </div>

        {/* Input de dirección URL Prominente */}
        <form onSubmit={handleNavigateSubmit} className="address-form">
          <div className="address-pill">
            <span className="address-lock">🔒</span>
            <input
              type="text"
              className="address-input"
              value={addressInput}
              onChange={(e) => setAddressInput(e.target.value)}
              placeholder="Introduce cualquier dirección web (ej: wikipedia.org o google.com)..."
            />
            {loading && <div className="address-spinner" />}
          </div>
          <button type="submit" className="btn btn-primary">
            Navegar
          </button>
        </form>

        {onToggleAgent && (
          <button
            type="button"
            onClick={onToggleAgent}
            className={`agent-toggle-btn ${isAgentOpen ? 'active' : ''}`}
            title="Abrir Copiloto IA"
          >
            <span>🤖</span>
            <span>Copiloto IA</span>
          </button>
        )}
      </div>

      {/* Área Principal de Renderizado / Inicio */}
      <div
        className="viewport-container"
        ref={containerRef}
        onWheel={screenshot ? handleWheel : undefined}
        tabIndex={screenshot ? 0 : undefined}
        onKeyDown={screenshot ? handleKeyDown : undefined}
      >
        {loadError ? (
          // Página de error dentro del visor (tarea 1.3): lo que pasó, con la
          // dirección pedida y una salida, en vez de dejar la página anterior
          // bajo una barra que ya muestra otra.
          <div className="load-error" role="alert">
            <span className="load-error-icon" aria-hidden="true">⚠️</span>
            <h2>No se ha podido cargar la página</h2>
            <p className="load-error-url">{loadError.url}</p>
            <p>{describeLoadError(loadError.message)}</p>
            <p className="load-error-detail">{loadError.message}</p>
            <div className="load-error-actions">
              <button type="button" className="btn btn-primary" onClick={onRetry}>
                Reintentar
              </button>
              {canGoBack && (
                <button type="button" className="btn" onClick={onBack}>
                  Volver atrás
                </button>
              )}
            </div>
          </div>
        ) : screenshot ? (
          <div style={{ position: 'relative', width: '100%', height: '100%', display: 'flex', justifyContent: 'center', alignItems: 'center' }}>
            <img
              ref={imgRef}
              src={`data:image/png;base64,${screenshot}`}
              alt="Browser Viewport"
              className="screenshot-image"
              onClick={handleImageClick}
            />

            {requiresJavascript && (
              <div className="js-required-overlay" role="status">
                <div className="js-required-card">
                  <span className="js-required-icon" aria-hidden="true">📄</span>
                  <h3>Esta página llega vacía</h3>
                  <p>
                    El servidor la ha enviado correctamente, pero sin contenido: lo construye
                    JavaScript en tu equipo, y el motor todavía no ejecuta ese tipo de código.
                  </p>
                  <p className="js-required-hint">
                    Le pasa a la mayoría de tiendas y aplicaciones web modernas. Las páginas que
                    envían su contenido ya hecho sí se ven bien.
                  </p>
                </div>
              </div>
            )}
          </div>
        ) : (
          /* PÁGINA DE INICIO / NUEVA PESTAÑA NATIVA (NTP) */
          <div className="ntp">
            <div className="ntp-mark">AI</div>

            <div>
              <h2 className="ntp-title">¿A dónde deseas navegar?</h2>
              <p className="ntp-subtitle">
                Introduce una URL en la barra superior o elige uno de los accesos rápidos a continuación:
              </p>
            </div>

            {/* Buscador Central de Inicio */}
            <form onSubmit={handleNavigateSubmit} className="ntp-search">
              <input
                type="text"
                value={addressInput}
                onChange={(e) => setAddressInput(e.target.value)}
                placeholder="Escribe una web (ej: wikipedia.org o google.com)..."
              />
              <button type="submit" className="btn btn-primary" style={{ padding: '14px 24px', fontSize: '1rem' }}>
                Ir a la Web
              </button>
            </form>

            {/* Enlaces de acceso rápido */}
            <div className="quick-links">
              {quickLinks.map((link) => (
                <button
                  key={link.name}
                  className="quick-link-card"
                  onClick={() => {
                    setAddressInput(link.url);
                    onManualNavigate(link.url);
                  }}
                >
                  <span className="quick-link-icon">{link.icon}</span>
                  <span className="quick-link-label">{link.name}</span>
                </button>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
};
export default BrowserViewport;
