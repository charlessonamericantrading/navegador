import React, { useState, useRef, useEffect } from 'react';
import { EXAMPLE_GOALS } from '../../onboarding/components/WelcomeGuide';

interface AgentStep {
  thought: string;
  action: string;
  url: string;
  title: string;
  finished: boolean;
  answer?: string;
}

// Debe coincidir con `max_steps` en backend/app/core/main.py
const MAX_STEPS = 15;

interface AgentConsoleProps {
  onStartAgent: (goal: string) => void;
  onStopAgent: () => void;
  agentStatus: 'idle' | 'running' | 'thinking' | 'finished' | 'error';
  agentMessage: string;
  steps: AgentStep[];
  presetGoal?: string;
  onPresetConsumed?: () => void;
}

export const AgentConsole: React.FC<AgentConsoleProps> = ({
  onStartAgent,
  onStopAgent,
  agentStatus,
  agentMessage,
  steps,
  presetGoal,
  onPresetConsumed
}) => {
  const [goalInput, setGoalInput] = useState('');
  const logEndRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  const isExecuting = agentStatus === 'running' || agentStatus === 'thinking';

  useEffect(() => {
    // Scroll al final cuando se añade un nuevo paso
    logEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [steps]);

  // Si el usuario eligió un ejemplo en la guía de bienvenida, precargarlo aquí
  // (sin lanzarlo solo: que confirme pulsando "Iniciar Agente")
  useEffect(() => {
    if (presetGoal) {
      setGoalInput(presetGoal);
      textareaRef.current?.focus();
      onPresetConsumed?.();
    }
  }, [presetGoal, onPresetConsumed]);

  const applyExample = (goal: string) => {
    setGoalInput(goal);
    textareaRef.current?.focus();
  };

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (goalInput.trim() && !isExecuting) {
      onStartAgent(goalInput.trim());
    }
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%', overflow: 'hidden' }}>
      {steps.length > 0 && (
        <div
          style={{
            display: 'flex',
            justifyContent: 'space-between',
            fontSize: '0.72rem',
            color: 'var(--text-dim)',
            marginBottom: '8px'
          }}
        >
          <span>Paso {steps.length} de {MAX_STEPS} como máximo</span>
          {steps.length >= MAX_STEPS - 2 && isExecuting && (
            <span style={{ color: 'var(--warning)' }}>Se acerca el límite de pasos</span>
          )}
        </div>
      )}

      {/* Historial de pasos del Agente */}
      <div className="agent-console-log">
        {steps.length === 0 ? (
          <div style={{
            flex: 1,
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            justifyContent: 'center',
            color: 'var(--text-dim)',
            padding: '20px',
            textAlign: 'center',
            gap: '16px'
          }}>
            <span style={{ fontSize: '0.82rem' }}>
              ¿No sabes qué pedirle? Prueba uno de estos ejemplos:
            </span>
            <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', width: '100%' }}>
              {EXAMPLE_GOALS.map(ex => (
                <button
                  key={ex.label}
                  type="button"
                  onClick={() => applyExample(ex.goal)}
                  className="btn"
                  style={{
                    textAlign: 'left',
                    background: 'var(--bg-card)',
                    border: '1px solid var(--border-light)',
                    color: 'var(--text-main)',
                    padding: '10px 12px',
                    display: 'flex',
                    alignItems: 'center',
                    gap: '10px'
                  }}
                >
                  <span>{ex.emoji}</span>
                  <span style={{ fontSize: '0.8rem' }}>{ex.label}</span>
                </button>
              ))}
            </div>
          </div>
        ) : (
          steps.map((step, idx) => (
            <div
              key={idx}
              className={`agent-log-step ${idx === steps.length - 1 ? 'active' : ''} ${step.finished ? 'finished' : ''}`}
            >
              <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                <span style={{
                  fontSize: '0.75rem',
                  fontWeight: 700,
                  textTransform: 'uppercase',
                  color: step.finished ? 'var(--success)' : 'var(--primary-light)'
                }}>
                  Paso {idx + 1}
                </span>
                <span className="log-time">Página: {step.title || 'Cargando...'}</span>
              </div>
              
              {step.thought && (
                <div className="log-thought">
                  💡 <strong>Pensamiento:</strong> {step.thought}
                </div>
              )}
              
              {step.action && (
                <div className="log-action">
                  ⚡ <strong>Acción:</strong> {step.action}
                </div>
              )}

              {step.finished && step.answer && (
                <div style={{
                  marginTop: '10px',
                  background: 'rgba(34, 197, 94, 0.08)',
                  border: '1px solid var(--success)',
                  borderRadius: '8px',
                  padding: '12px',
                  fontSize: '0.88rem',
                  color: 'var(--text-main)',
                  lineHeight: '1.4'
                }}>
                  🎉 <strong>Respuesta del Agente:</strong> {step.answer}
                </div>
              )}
            </div>
          ))
        )}
        <div ref={logEndRef} />
      </div>

      {/* Indicador de Estado actual */}
      {isExecuting && (
        <div style={{
          display: 'flex',
          alignItems: 'center',
          gap: '10px',
          background: 'rgba(234, 179, 8, 0.05)',
          border: '1px solid rgba(234, 179, 8, 0.15)',
          borderRadius: '8px',
          padding: '10px 14px',
          fontSize: '0.82rem',
          color: 'var(--warning)',
          marginBottom: '12px'
        }}>
          <div className="status-dot thinking" />
          <span>
            {agentStatus === 'thinking' 
              ? 'El agente está decidiendo la siguiente acción...' 
              : 'El agente está interactuando con la página...'}
          </span>
        </div>
      )}

      {agentStatus === 'error' && (
        <div style={{
          background: 'rgba(239, 68, 68, 0.08)',
          border: '1px solid var(--error)',
          borderRadius: '8px',
          padding: '10px 14px',
          fontSize: '0.82rem',
          color: 'var(--error)',
          marginBottom: '12px',
          lineHeight: '1.4'
        }}>
          ❌ {agentMessage}
        </div>
      )}

      {agentStatus === 'finished' && !steps.some(s => s.finished) && (
        <div style={{
          background: 'rgba(34, 197, 94, 0.08)',
          border: '1px solid var(--success)',
          borderRadius: '8px',
          padding: '10px 14px',
          fontSize: '0.82rem',
          color: 'var(--success)',
          marginBottom: '12px'
        }}>
          ✅ {agentMessage}
        </div>
      )}

      {/* Consola de Entrada */}
      <form onSubmit={handleSubmit} className="prompt-container">
        <textarea
          ref={textareaRef}
          className="prompt-textarea"
          placeholder="Escribe qué quieres que haga el agente, en tus propias palabras... (ej: 'Busca en Wikipedia qué es la fotosíntesis')"
          value={goalInput}
          onChange={(e) => setGoalInput(e.target.value)}
          disabled={isExecuting}
        />
        <div className="prompt-actions">
          <span style={{ fontSize: '0.75rem', color: 'var(--text-dim)' }}>
            {isExecuting ? 'Ejecución autónoma activa' : 'Pulsa Ejecutar para iniciar'}
          </span>
          {isExecuting ? (
            <button type="button" onClick={onStopAgent} className="btn btn-stop">
              Detener Agente
            </button>
          ) : (
            <button
              type="submit"
              disabled={!goalInput.trim()}
              className="btn btn-primary"
              style={{ opacity: goalInput.trim() ? 1 : 0.6 }}
            >
              Iniciar Agente
            </button>
          )}
        </div>
      </form>
    </div>
  );
};
export default AgentConsole;
