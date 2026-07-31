import React from 'react';

interface SettingsPanelProps {
  apiKey: string;
  setApiKey: (val: string) => void;
  agentMode: string;
  setAgentMode: (val: string) => void;
  agentSpeed: number;
  setAgentSpeed: (val: number) => void;
}

export const SettingsPanel: React.FC<SettingsPanelProps> = ({
  apiKey,
  setApiKey,
  agentMode,
  setAgentMode,
  agentSpeed,
  setAgentSpeed
}) => {
  return (
    <div className="app-sidebar">
      <div>
        <h3 style={{ marginBottom: '16px', fontSize: '1rem', fontWeight: 600, letterSpacing: '-0.2px' }}>
          Ajustes
        </h3>

        <div className="form-group">
          <label>Modo de funcionamiento</label>
          <select value={agentMode} onChange={(e) => setAgentMode(e.target.value)}>
            <option value="simulation">🤖 Modo gratuito (sin clave, ideal para probar)</option>
            <option value="gemini">✨ Modo IA real (con tu clave de Google)</option>
          </select>
          <span style={{ fontSize: '0.75rem', color: 'var(--text-dim)', marginTop: '4px', lineHeight: 1.3 }}>
            {agentMode === 'gemini'
              ? 'El agente razona con la IA Gemini de Google para tareas más complejas.'
              : 'El agente sigue reglas sencillas y predecibles, sin conectarse a ningún servicio externo.'}
          </span>
        </div>

        {agentMode === 'gemini' && (
          <div className="form-group">
            <label>Tu clave de API de Gemini</label>
            <input
              type="password"
              placeholder="Pega aquí tu clave..."
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
            />
            <span style={{ fontSize: '0.75rem', color: 'var(--text-dim)', marginTop: '4px', lineHeight: 1.3 }}>
              Tu clave se guarda únicamente en este ordenador, nunca sale de aquí salvo hacia Google
              al usar el modo IA real. Consíguela gratis en{' '}
              <span style={{ color: 'var(--accent)' }}>aistudio.google.com/apikey</span>.
            </span>

            <div
              style={{
                marginTop: '10px',
                background: 'rgba(234, 179, 8, 0.06)',
                border: '1px solid rgba(234, 179, 8, 0.2)',
                borderRadius: '8px',
                padding: '10px 12px',
                display: 'flex',
                flexDirection: 'column',
                gap: '6px'
              }}
            >
              <span style={{ fontSize: '0.75rem', color: 'var(--warning)', fontWeight: 600 }}>
                Antes de activarlo, ten en cuenta:
              </span>
              <span style={{ fontSize: '0.72rem', color: 'var(--text-dim)', lineHeight: 1.5 }}>
                🔎 En este modo, el texto de cada página (títulos, botones, enlaces) se envía a los
                servidores de Google para que la IA decida qué hacer. No se envían capturas de
                pantalla ni tus contraseñas.
              </span>
              <span style={{ fontSize: '0.72rem', color: 'var(--text-dim)', lineHeight: 1.5 }}>
                💳 Google ofrece un nivel gratuito, pero un uso muy intensivo podría generar
                costes según tu plan. Revisa tu consumo en Google AI Studio si tienes dudas.
              </span>
            </div>
          </div>
        )}

        <div className="form-group">
          <label>Velocidad del agente ({agentSpeed}s entre pasos)</label>
          <select
            value={agentSpeed}
            onChange={(e) => setAgentSpeed(Number(e.target.value))}
          >
            <option value={1}>⚡ Rápida</option>
            <option value={2}>🐢 Media (recomendada)</option>
            <option value={3}>⏳ Lenta (más fácil de seguir)</option>
          </select>
        </div>
      </div>

      <div style={{ marginTop: 'auto', borderTop: '1px solid var(--border-light)', paddingTop: '16px' }}>
        <h4 style={{ fontSize: '0.8rem', color: 'var(--text-muted)', marginBottom: '8px' }}>
          ¿Cómo funciona?
        </h4>
        <ul style={{ fontSize: '0.78rem', color: 'var(--text-dim)', paddingLeft: '16px', lineHeight: 1.5 }}>
          <li>Escribe lo que quieres lograr en el panel de la derecha.</li>
          <li>Se abre un navegador de pruebas independiente, separado del tuyo.</li>
          <li>El agente identifica los botones, enlaces y campos de la página.</li>
          <li>Va haciendo clics y escribiendo hasta cumplir tu objetivo, paso a paso y a la vista.</li>
        </ul>
      </div>
    </div>
  );
};
export default SettingsPanel;
