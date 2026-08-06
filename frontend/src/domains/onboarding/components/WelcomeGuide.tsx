import React, { useEffect, useState } from 'react';

export interface ExampleGoal {
  emoji: string;
  label: string;
  goal: string;
}

export const EXAMPLE_GOALS: ExampleGoal[] = [
  {
    emoji: '📚',
    label: 'Resumir un tema en Wikipedia',
    goal: 'Busca en Wikipedia qué es la inteligencia artificial y resume la primera frase.'
  },
  {
    emoji: '🔎',
    label: 'Buscar un dato rápido en Google',
    goal: 'Busca en Google cuál es la capital de Australia.'
  },
  {
    emoji: '🧑‍🔬',
    label: 'Conocer a un personaje histórico',
    goal: 'Ve a Wikipedia y cuéntame en una frase quién fue Alan Turing.'
  }
];

interface WelcomeGuideProps {
  onFinish: (chosenExample?: string) => void;
}

const TOTAL_STEPS = 3;

export const WelcomeGuide: React.FC<WelcomeGuideProps> = ({ onFinish }) => {
  const [step, setStep] = useState(0);

  const next = () => setStep(s => Math.min(s + 1, TOTAL_STEPS - 1));
  const back = () => setStep(s => Math.max(s - 1, 0));

  // Permitir cerrar la guía con Escape, como se espera de cualquier diálogo modal
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onFinish();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [onFinish]);

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        background: 'rgba(5, 7, 12, 0.72)',
        backdropFilter: 'blur(4px)',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        zIndex: 100
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Guía de bienvenida"
        className="glass-card"
        style={{
          width: 'min(560px, 90vw)',
          background: 'hsl(222, 24%, 9%)',
          border: '1px solid var(--border-light)',
          padding: '32px',
          display: 'flex',
          flexDirection: 'column',
          gap: '20px'
        }}
      >
        {/* Indicador de pasos */}
        <div style={{ display: 'flex', gap: '6px' }}>
          {Array.from({ length: TOTAL_STEPS }).map((_, i) => (
            <div
              key={i}
              style={{
                height: '4px',
                flex: 1,
                borderRadius: '4px',
                background: i <= step ? 'var(--primary)' : 'var(--border-light)',
                transition: 'var(--transition)'
              }}
            />
          ))}
        </div>

        {step === 0 && (
          <>
            <div style={{ fontSize: '2rem' }}>👋</div>
            <h2 style={{ fontSize: '1.3rem', fontWeight: 700 }}>Bienvenido a Navegador IA</h2>
            <p style={{ color: 'var(--text-muted)', lineHeight: 1.6, fontSize: '0.95rem' }}>
              Esta app abre un <strong>navegador de pruebas independiente</strong> (no tu navegador
              habitual, ni tus contraseñas o pestañas) y deja que un agente de IA navegue por
              internet siguiendo instrucciones que tú escribes en español sencillo.
            </p>
          </>
        )}

        {step === 1 && (
          <>
            <div style={{ fontSize: '2rem' }}>⚙️</div>
            <h2 style={{ fontSize: '1.3rem', fontWeight: 700 }}>Dos formas de usarlo</h2>
            <div style={{ display: 'flex', flexDirection: 'column', gap: '12px' }}>
              <div className="glass-card" style={{ padding: '14px', border: '1px solid var(--border-light)' }}>
                <strong style={{ fontSize: '0.9rem' }}>🤖 Modo gratuito</strong>
                <p style={{ color: 'var(--text-dim)', fontSize: '0.82rem', marginTop: '4px', lineHeight: 1.5 }}>
                  Funciona nada más abrir la app, sin registrarte ni configurar nada. Sigue reglas
                  sencillas, ideal para probar cómo funciona.
                </p>
              </div>
              <div className="glass-card" style={{ padding: '14px', border: '1px solid var(--border-light)' }}>
                <strong style={{ fontSize: '0.9rem' }}>✨ Modo IA real (Gemini)</strong>
                <p style={{ color: 'var(--text-dim)', fontSize: '0.82rem', marginTop: '4px', lineHeight: 1.5 }}>
                  Razona de verdad con IA de Google. Es opcional y requiere tu propia clave gratuita
                  de Google AI Studio, que se guarda solo en tu ordenador.
                </p>
              </div>
            </div>
            <p style={{ color: 'var(--text-dim)', fontSize: '0.78rem', lineHeight: 1.5 }}>
              Recomendación: empieza en modo gratuito y activa el modo IA real cuando quieras
              tareas más complejas.
            </p>
          </>
        )}

        {step === 2 && (
          <>
            <div style={{ fontSize: '2rem' }}>🚀</div>
            <h2 style={{ fontSize: '1.3rem', fontWeight: 700 }}>Prueba tu primer ejemplo</h2>
            <p style={{ color: 'var(--text-muted)', fontSize: '0.9rem', lineHeight: 1.5 }}>
              Así son las instrucciones que puedes darle al agente. Cierra esta guía cuando quieras
              para volver al navegador.
            </p>
            <div style={{ display: 'flex', flexDirection: 'column', gap: '8px' }}>
              {EXAMPLE_GOALS.map(ex => (
                <button
                  key={ex.label}
                  onClick={() => onFinish(ex.goal)}
                  className="btn"
                  style={{
                    textAlign: 'left',
                    background: 'var(--bg-card)',
                    border: '1px solid var(--border-light)',
                    color: 'var(--text-main)',
                    padding: '12px 14px',
                    display: 'flex',
                    alignItems: 'center',
                    gap: '10px'
                  }}
                >
                  <span style={{ fontSize: '1.1rem' }}>{ex.emoji}</span>
                  <span style={{ fontSize: '0.85rem' }}>{ex.label}</span>
                </button>
              ))}
            </div>
          </>
        )}

        {/* Navegación entre pasos */}
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginTop: '8px' }}>
          <button
            onClick={() => onFinish()}
            className="btn"
            style={{ background: 'transparent', color: 'var(--text-dim)', padding: '8px 4px' }}
          >
            {step === TOTAL_STEPS - 1 ? 'Cerrar y explorar por mi cuenta' : 'Omitir guía'}
          </button>

          <div style={{ display: 'flex', gap: '8px' }}>
            {step > 0 && (
              <button
                onClick={back}
                className="btn"
                style={{ background: 'rgba(255,255,255,0.06)', color: 'var(--text-main)' }}
              >
                Atrás
              </button>
            )}
            {step < TOTAL_STEPS - 1 && (
              <button onClick={next} className="btn btn-primary">
                Siguiente
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};

export default WelcomeGuide;
