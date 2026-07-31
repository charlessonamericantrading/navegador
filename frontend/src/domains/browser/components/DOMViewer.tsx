import React, { useState } from 'react';

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

interface DOMViewerProps {
  elements: InteractiveElement[];
  hoveredElementId: number | null;
  setHoveredElementId: (id: number | null) => void;
  onElementSelect?: (element: InteractiveElement) => void;
}

export const DOMViewer: React.FC<DOMViewerProps> = ({
  elements,
  hoveredElementId,
  setHoveredElementId,
  onElementSelect
}) => {
  const [search, setSearch] = useState('');

  const filtered = elements.filter(el => {
    const query = search.toLowerCase();
    return (
      el.id.toString() === query ||
      el.tagName.toLowerCase().includes(query) ||
      el.text.toLowerCase().includes(query) ||
      (el.attributes.placeholder && el.attributes.placeholder.toLowerCase().includes(query)) ||
      (el.attributes.href && el.attributes.href.toLowerCase().includes(query))
    );
  });

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%', overflow: 'hidden' }}>
      <p style={{ fontSize: '0.78rem', color: 'var(--text-dim)', lineHeight: 1.4, marginBottom: '10px' }}>
        Esto es todo lo que el agente puede detectar y pulsar en la página actual: botones,
        enlaces y campos de texto.
      </p>
      <div style={{ marginBottom: '12px' }}>
        <input
          type="text"
          placeholder="Filtrar elementos (ej: botón, Buscar)..."
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          style={{
            width: '100%',
            background: 'var(--bg-input)',
            border: '1px solid var(--border-light)',
            borderRadius: '6px',
            padding: '8px 12px',
            color: 'var(--text-main)',
            fontSize: '0.82rem',
            outline: 'none'
          }}
        />
      </div>

      <div className="dom-tree-list">
        {filtered.length === 0 ? (
          <div style={{ textAlign: 'center', color: 'var(--text-dim)', padding: '24px 0', fontSize: '0.8rem' }}>
            No se encontraron elementos interactivos.
          </div>
        ) : (
          filtered.map(el => (
            <div
              key={el.id}
              role="button"
              tabIndex={0}
              className={`dom-tree-item ${hoveredElementId === el.id ? 'active' : ''}`}
              onMouseEnter={() => setHoveredElementId(el.id)}
              onMouseLeave={() => setHoveredElementId(null)}
              onFocus={() => setHoveredElementId(el.id)}
              onBlur={() => setHoveredElementId(null)}
              onClick={() => onElementSelect && onElementSelect(el)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  onElementSelect && onElementSelect(el);
                }
              }}
              style={hoveredElementId === el.id ? { borderColor: 'var(--accent)', background: 'rgba(6, 182, 212, 0.05)' } : {}}
            >
              <div className="dom-tree-id">[{el.id}]</div>
              <div className="dom-tree-details">
                <span className="dom-tree-tag">
                  {el.tagName.toLowerCase()}
                  {el.attributes.type ? ` (${el.attributes.type})` : ''}
                </span>
                <span className="dom-tree-text">
                  {el.text || <span style={{ color: 'var(--text-dim)', fontStyle: 'italic' }}>sin etiqueta</span>}
                </span>
                {el.attributes.placeholder && (
                  <span style={{ fontSize: '0.7rem', color: 'var(--accent)' }}>
                    Placeholder: "{el.attributes.placeholder}"
                  </span>
                )}
                {el.attributes.href && (
                  <span style={{ fontSize: '0.7rem', color: 'var(--text-dim)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', maxWidth: '220px' }}>
                    Link: {el.attributes.href}
                  </span>
                )}
              </div>
            </div>
          ))
        )}
      </div>
    </div>
  );
};
export default DOMViewer;
