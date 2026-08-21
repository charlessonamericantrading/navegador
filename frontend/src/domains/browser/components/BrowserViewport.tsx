import React, { useState, useRef, useEffect } from 'react';

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
  onManualNavigate: (url: string) => void;
  onManualClick: (x: number, y: number) => void;
  onManualType: (x: number, y: number, text: string) => void;
  onManualResize: (width: number, height: number) => void;
  onManualScroll: (dy: number) => void;
  loading: boolean;
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
}

export const BrowserViewport: React.FC<BrowserViewportProps> = ({
  screenshot,
  url,
  elements,
  onManualNavigate,
  onManualClick,
  onManualType,
  onManualResize,
  onManualScroll,
  loading,
  canGoBack,
  canGoForward,
  onBack,
  onForward,
  tabs,
  activeTabId,
  onNewTab,
  onSwitchTab,
  onCloseTab
}) => {
  const [addressInput, setAddressInput] = useState(url || 'https://www.google.com');
  const [inputTextPopup, setInputTextPopup] = useState<{
    mx: number;
    my: number;
    text: string;
    placeholder: string;
  } | null>(null);

  const containerRef = useRef<HTMLDivElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const popupInputRef = useRef<HTMLInputElement>(null);

  // Rectángulo real (en píxeles, relativo al "stage") que ocupa la imagen del
  // screenshot dentro de su contenedor. La imagen se ajusta a su caja con
  // maxWidth/maxHeight y se centra con flexbox, así que salvo que el panel
  // tenga exactamente proporción 16:9 quedan franjas vacías a los lados o
  // arriba/abajo. Las insignias/resaltados deben posicionarse relativas a
  // ESTE rectángulo, no al 100%/100% del contenedor (ver bug de desalineación).
  const [imgRect, setImgRect] = useState<{ left: number; top: number; width: number; height: number } | null>(null);

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

  useEffect(() => {
    const updateImgRect = () => {
      if (!imgRef.current || !stageRef.current) return;
      // Antes de que la imagen (data URI) termine de decodificarse, su caja
      // puede medir 0x0 durante un instante; publicar eso colapsaría todas las
      // insignias en el mismo punto hasta el próximo recálculo. Se espera a
      // "complete" y se deja que el onLoad de la imagen dispare la primera
      // medición válida.
      if (!imgRef.current.complete || imgRef.current.naturalWidth === 0) return;
      const imgBox = imgRef.current.getBoundingClientRect();
      const stageBox = stageRef.current.getBoundingClientRect();
      setImgRect({
        left: imgBox.left - stageBox.left,
        top: imgBox.top - stageBox.top,
        width: imgBox.width,
        height: imgBox.height
      });
    };

    updateImgRect();

    const resizeObserver = new ResizeObserver(updateImgRect);
    if (imgRef.current) resizeObserver.observe(imgRef.current);
    window.addEventListener('resize', updateImgRect);

    return () => {
      resizeObserver.disconnect();
      window.removeEventListener('resize', updateImgRect);
    };
  }, [screenshot]);

  // Convierte una coordenada del modelo (espacio de píxeles real de la
  // captura recibida, no un tamaño fijo) a píxeles dentro del "stage",
  // usando el rectángulo real de la imagen.
  const toStagePx = (mx: number, my: number) => {
    if (!imgRect || !imgRef.current) return { left: 0, top: 0 };
    const naturalWidth = imgRef.current.naturalWidth || imgRect.width;
    const naturalHeight = imgRef.current.naturalHeight || imgRect.height;
    return {
      left: imgRect.left + (mx / naturalWidth) * imgRect.width,
      top: imgRect.top + (my / naturalHeight) * imgRect.height
    };
  };

  useEffect(() => {
    if (url) {
      setAddressInput(url);
    }
  }, [url]);

  useEffect(() => {
    if (inputTextPopup && popupInputRef.current) {
      popupInputRef.current.focus();
    }
  }, [inputTextPopup]);

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
    if (addressInput.trim() && !loading) {
      onManualNavigate(resolveAddressTarget(addressInput));
    }
  };

  const handleImageClick = (e: React.MouseEvent<HTMLImageElement>) => {
    if (inputTextPopup) {
      setInputTextPopup(null);
      return;
    }

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

    const clickedInputEl = elements.find(el => {
      const { x, y, width, height } = el.rect;
      const isInside = (
        mappedX >= x &&
        mappedX <= x + width &&
        mappedY >= y &&
        mappedY <= y + height
      );
      return isInside && (el.tag_name === 'input' || el.tag_name === 'textarea');
    });

    if (clickedInputEl) {
      setInputTextPopup({
        mx: clickedInputEl.rect.x + clickedInputEl.rect.width / 2,
        my: clickedInputEl.rect.y + clickedInputEl.rect.height / 2,
        text: '',
        placeholder: clickedInputEl.attributes.placeholder || 'Escribe texto...'
      });
    } else {
      onManualClick(mappedX, mappedY);
    }
  };

  const handleWheel = (e: React.WheelEvent) => {
    // El popup de escritura se posiciona sobre unas coordenadas de la página
    // que dejan de ser válidas en cuanto ésta se desplaza.
    if (inputTextPopup) {
      setInputTextPopup(null);
    }

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

  const handlePopupSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (inputTextPopup) {
      onManualType(inputTextPopup.mx, inputTextPopup.my, inputTextPopup.text);
      setInputTextPopup(null);
    }
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
          <button
            onClick={() => onManualNavigate(url || 'https://www.google.com')}
            className="btn icon-btn"
            title="Recargar página"
            aria-label="Recargar página"
          >
            ↻
          </button>
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
      </div>

      {/* Área Principal de Renderizado / Inicio */}
      <div className="viewport-container" ref={containerRef} onWheel={screenshot ? handleWheel : undefined}>
        {screenshot ? (
          <div ref={stageRef} style={{ position: 'relative', width: '100%', height: '100%', display: 'flex', justifyContent: 'center', alignItems: 'center' }}>
            <img
              ref={imgRef}
              src={`data:image/jpeg;base64,${screenshot}`}
              alt="Browser Viewport"
              className="screenshot-image"
              onClick={handleImageClick}
              onLoad={() => {
                if (imgRef.current && stageRef.current) {
                  const imgBox = imgRef.current.getBoundingClientRect();
                  const stageBox = stageRef.current.getBoundingClientRect();
                  setImgRect({
                    left: imgBox.left - stageBox.left,
                    top: imgBox.top - stageBox.top,
                    width: imgBox.width,
                    height: imgBox.height
                  });
                }
              }}
            />

            {inputTextPopup && (() => {
              const { left, top } = toStagePx(inputTextPopup.mx, inputTextPopup.my);
              return (
              <form
                onSubmit={handlePopupSubmit}
                className="inline-type-popup"
                style={{ left: `${left}px`, top: `${top}px` }}
                onClick={(e) => e.stopPropagation()}
              >
                <input
                  ref={popupInputRef}
                  type="text"
                  placeholder={inputTextPopup.placeholder}
                  value={inputTextPopup.text}
                  onChange={(e) => setInputTextPopup({ ...inputTextPopup, text: e.target.value })}
                />
                <button type="submit" className="btn btn-primary" style={{ padding: '8px 14px' }}>
                  Enviar
                </button>
              </form>
              );
            })()}
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
