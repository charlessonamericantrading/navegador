import React, { useState, useRef, useEffect } from 'react';

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

interface BrowserViewportProps {
  screenshot: string;
  url: string;
  elements: InteractiveElement[];
  onManualNavigate: (url: string) => void;
  onManualClick: (x: number, y: number) => void;
  onManualType: (x: number, y: number, text: string) => void;
  hoveredElementId: number | null;
  setHoveredElementId: (id: number | null) => void;
  loading: boolean;
}

export const BrowserViewport: React.FC<BrowserViewportProps> = ({
  screenshot,
  url,
  elements,
  onManualNavigate,
  onManualClick,
  onManualType,
  hoveredElementId,
  setHoveredElementId,
  loading
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

  // Convierte una coordenada del modelo (espacio 1280x720 del backend) a
  // píxeles dentro del "stage", usando el rectángulo real de la imagen.
  const toStagePx = (mx: number, my: number) => {
    if (!imgRect) return { left: 0, top: 0 };
    return {
      left: imgRect.left + (mx / 1280) * imgRect.width,
      top: imgRect.top + (my / 720) * imgRect.height
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

  const handleNavigateSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (addressInput.trim() && !loading) {
      let target = addressInput.trim();
      if (!target.startsWith('http://') && !target.startsWith('https://')) {
        target = 'https://' + target;
      }
      onManualNavigate(target);
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

    const scaleX = 1280 / rect.width;
    const scaleY = 720 / rect.height;
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
      return isInside && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA');
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

  const handlePopupSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (inputTextPopup) {
      onManualType(inputTextPopup.mx, inputTextPopup.my, inputTextPopup.text);
      setInputTextPopup(null);
    }
  };

  const hoveredElement = elements.find(el => el.id === hoveredElementId);

  const quickLinks = [
    { name: 'Google', url: 'https://www.google.com', icon: '🔍' },
    { name: 'Wikipedia', url: 'https://es.wikipedia.org', icon: '📚' },
    { name: 'GitHub', url: 'https://github.com', icon: '💻' },
    { name: 'Noticias', url: 'https://news.google.com', icon: '📰' },
  ];

  return (
    <div className="browser-frame" style={{ display: 'flex', flexDirection: 'column', height: '100%', background: '#f8fafc' }}>
      {/* Barra de Navegación del Navegador (Nivel Superior Prominente) */}
      <div className="browser-bar" style={{
        display: 'flex',
        alignItems: 'center',
        gap: '12px',
        padding: '10px 16px',
        background: '#ffffff',
        borderBottom: '1px solid #e2e8f0',
        boxShadow: '0 2px 8px rgba(0,0,0,0.04)'
      }}>
        {/* Botones de control de navegación */}
        <div style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
          <button
            onClick={() => onManualNavigate(url || 'https://www.google.com')}
            className="btn"
            style={{ padding: '6px 10px', background: '#f1f5f9', border: '1px solid #cbd5e1', color: '#334155' }}
            title="Recargar página"
          >
            ↻
          </button>
        </div>

        {/* Input de dirección URL Prominente */}
        <form onSubmit={handleNavigateSubmit} style={{ flex: 1, display: 'flex', alignItems: 'center', gap: '8px' }}>
          <div style={{
            flex: 1,
            display: 'flex',
            alignItems: 'center',
            gap: '8px',
            background: '#f8fafc',
            border: '1.5px solid #cbd5e1',
            borderRadius: '10px',
            padding: '6px 14px',
            boxShadow: 'inset 0 1px 2px rgba(0,0,0,0.03)'
          }}>
            <span style={{ fontSize: '0.85rem', color: '#64748b' }}>🔒</span>
            <input
              type="text"
              value={addressInput}
              onChange={(e) => setAddressInput(e.target.value)}
              placeholder="Introduce cualquier dirección web (ej: wikipedia.org o google.com)..."
              style={{
                width: '100%',
                background: 'transparent',
                border: 'none',
                color: '#0f172a',
                fontSize: '0.9rem',
                fontWeight: 500,
                outline: 'none'
              }}
            />
            {loading && (
              <div style={{
                width: '16px',
                height: '16px',
                border: '2px solid #cbd5e1',
                borderTopColor: '#6d28d9',
                borderRadius: '50%',
                animation: 'spin 0.8s linear infinite'
              }} />
            )}
          </div>
          <button type="submit" className="btn btn-primary" style={{ padding: '8px 18px', fontWeight: 600 }}>
            Navegar
          </button>
        </form>
      </div>

      {/* Área Principal de Renderizado / Inicio */}
      <div className="viewport-container" ref={containerRef} style={{ flex: 1, position: 'relative', overflow: 'hidden', background: '#f1f5f9', display: 'flex', justifyContent: 'center', alignItems: 'center' }}>
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
              style={{ maxWidth: '100%', maxHeight: '100%', objectFit: 'contain', cursor: 'crosshair', borderRadius: '8px', boxShadow: '0 8px 30px rgba(0,0,0,0.1)' }}
            />

            {imgRect && elements.map(el => {
              const cx = el.rect.x + el.rect.width / 2;
              const cy = el.rect.y + el.rect.height / 2;
              const { left, top } = toStagePx(cx, cy);

              return (
                <div
                  key={el.id}
                  className="overlay-badge"
                  style={{
                    position: 'absolute',
                    left: `${left}px`,
                    top: `${top}px`,
                    border: hoveredElementId === el.id ? '2px solid #6d28d9' : '1.5px solid white',
                    transform: hoveredElementId === el.id ? 'translate(-50%, -50%) scale(1.25)' : 'translate(-50%, -50%)',
                    background: '#6d28d9',
                    color: 'white',
                    padding: '2px 6px',
                    borderRadius: '4px',
                    fontSize: '0.7rem',
                    fontWeight: 700,
                    cursor: 'pointer',
                    boxShadow: '0 2px 6px rgba(0,0,0,0.3)'
                  }}
                  onMouseEnter={() => setHoveredElementId(el.id)}
                  onMouseLeave={() => setHoveredElementId(null)}
                  onClick={(e) => {
                    e.stopPropagation();
                    if (loading) return;
                    if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {
                      setInputTextPopup({
                        mx: cx,
                        my: cy,
                        text: '',
                        placeholder: el.attributes.placeholder || 'Escribe texto...'
                      });
                    } else {
                      onManualClick(cx, cy);
                    }
                  }}
                >
                  {el.id}
                </div>
              );
            })}

            {hoveredElement && imgRect && (
              <div
                style={{
                  position: 'absolute',
                  left: `${imgRect.left + (hoveredElement.rect.x / 1280) * imgRect.width}px`,
                  top: `${imgRect.top + (hoveredElement.rect.y / 720) * imgRect.height}px`,
                  width: `${(hoveredElement.rect.width / 1280) * imgRect.width}px`,
                  height: `${(hoveredElement.rect.height / 720) * imgRect.height}px`,
                  border: '2px dashed #6d28d9',
                  pointerEvents: 'none'
                }}
              />
            )}

            {inputTextPopup && (() => {
              const { left, top } = toStagePx(inputTextPopup.mx, inputTextPopup.my);
              return (
              <form
                onSubmit={handlePopupSubmit}
                style={{
                  position: 'absolute',
                  left: `${left}px`,
                  top: `${top}px`,
                  transform: 'translate(-50%, -120%)',
                  background: '#ffffff',
                  border: '1px solid #cbd5e1',
                  borderRadius: '10px',
                  padding: '10px',
                  display: 'flex',
                  gap: '8px',
                  zIndex: 20,
                  boxShadow: '0 10px 25px rgba(0,0,0,0.15)',
                  minWidth: '260px'
                }}
                onClick={(e) => e.stopPropagation()}
              >
                <input
                  ref={popupInputRef}
                  type="text"
                  placeholder={inputTextPopup.placeholder}
                  value={inputTextPopup.text}
                  onChange={(e) => setInputTextPopup({ ...inputTextPopup, text: e.target.value })}
                  style={{ flex: 1, padding: '8px 12px', borderRadius: '6px', border: '1px solid #cbd5e1' }}
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
          <div style={{
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            justifyContent: 'center',
            padding: '40px',
            maxWidth: '640px',
            width: '100%',
            textAlign: 'center',
            gap: '24px'
          }}>
            <div style={{
              width: '64px',
              height: '64px',
              background: 'linear-gradient(135deg, #6d28d9, #0284c7)',
              borderRadius: '18px',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              fontSize: '1.8rem',
              color: 'white',
              fontWeight: 800,
              boxShadow: '0 10px 25px rgba(109,40,217,0.3)'
            }}>
              AI
            </div>

            <div>
              <h2 style={{ fontSize: '1.75rem', fontWeight: 800, color: '#0f172a', marginBottom: '8px' }}>
                ¿A dónde deseas navegar?
              </h2>
              <p style={{ color: '#64748b', fontSize: '0.95rem' }}>
                Introduce una URL en la barra superior o elige uno de los accesos rápidos a continuación:
              </p>
            </div>

            {/* Buscador Central de Inicio */}
            <form onSubmit={handleNavigateSubmit} style={{ width: '100%', display: 'flex', gap: '8px' }}>
              <input
                type="text"
                value={addressInput}
                onChange={(e) => setAddressInput(e.target.value)}
                placeholder="Escribe una web (ej: wikipedia.org o google.com)..."
                style={{
                  flex: 1,
                  padding: '14px 18px',
                  borderRadius: '12px',
                  border: '1.5px solid #cbd5e1',
                  fontSize: '1rem',
                  boxShadow: '0 4px 12px rgba(0,0,0,0.04)'
                }}
              />
              <button type="submit" className="btn btn-primary" style={{ padding: '14px 24px', fontSize: '1rem' }}>
                Ir a la Web
              </button>
            </form>

            {/* Enlaces de acceso rápido */}
            <div style={{ display: 'grid', gridTemplateColumns: 'repeat(4, 1fr)', gap: '12px', width: '100%', marginTop: '12px' }}>
              {quickLinks.map((link) => (
                <button
                  key={link.name}
                  onClick={() => {
                    setAddressInput(link.url);
                    onManualNavigate(link.url);
                  }}
                  style={{
                    display: 'flex',
                    flexDirection: 'column',
                    alignItems: 'center',
                    gap: '8px',
                    padding: '16px',
                    background: '#ffffff',
                    border: '1px solid #e2e8f0',
                    borderRadius: '12px',
                    cursor: 'pointer',
                    transition: 'all 0.2s ease',
                    boxShadow: '0 2px 6px rgba(0,0,0,0.03)'
                  }}
                >
                  <span style={{ fontSize: '1.5rem' }}>{link.icon}</span>
                  <span style={{ fontSize: '0.85rem', fontWeight: 600, color: '#334155' }}>{link.name}</span>
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
