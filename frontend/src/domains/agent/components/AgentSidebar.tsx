import React, { useState, useEffect, useRef } from 'react';
import { AgentCancelledError, AgentOrchestrator, cancellableDelay, shouldStopAfterFailures, type AgentStepResult, type BrowserInterface } from '../AgentOrchestrator';
import { electronModelProvider } from '../modelProvider';

// Donde vivia la clave antes de la Fase 53. Solo se lee una vez, para
// migrarla al proceso principal, y se borra.
const LEGACY_KEY_STORAGE = 'gemini_api_key';

interface AgentSidebarProps {
  isOpen: boolean;
  onClose: () => void;
  browserInterface: BrowserInterface;
}

export const AgentSidebar: React.FC<AgentSidebarProps> = ({ isOpen, onClose, browserInterface }) => {
  const [goal, setGoal] = useState('');
  const [mode, setMode] = useState<'simulation' | 'gemini'>('simulation');
  // La clave ya no vive aqui (plan H04): el renderer solo conoce su estado.
  // `keyDraft` es lo que se esta escribiendo en el campo, y se vacia al
  // entregarlo al proceso principal.
  const [credentialStatus, setCredentialStatus] = useState<AiCredentialStatus | null>(null);
  const [keyDraft, setKeyDraft] = useState('');
  const [showSettings, setShowSettings] = useState(false);
  const [isRunning, setIsRunning] = useState(false);
  const [currentStep, setCurrentStep] = useState(0);
  const [statusMessage, setStatusMessage] = useState<string>('Listo para recibir instrucciones.');
  const [stepsHistory, setStepsHistory] = useState<AgentStepResult[]>([]);
  const [finalAnswer, setFinalAnswer] = useState<string | null>(null);

  // Una ejecución = un controlador. «Detener» lo aborta, y eso llega a la
  // petición al modelo y a cada punto de control del orquestador. Que sea
  // por ejecución (y no una bandera compartida) impide que un paso de una
  // ejecución ya detenida toque el estado de la siguiente.
  const runControllerRef = useRef<AbortController | null>(null);
  const orchestratorRef = useRef<AgentOrchestrator | null>(null);
  const historyEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    orchestratorRef.current = new AgentOrchestrator(browserInterface, electronModelProvider());
  }, [browserInterface]);

  // Estado de la clave y migracion unica desde `localStorage`. La clave vieja
  // solo se borra cuando el proceso principal confirma que la tiene; si no se
  // puede entregar (fuera de Electron), se borra igualmente: dejarla en el
  // almacenamiento del renderer es justo lo que esta fase elimina, y el modo
  // Gemini tampoco funciona fuera de la aplicacion de escritorio.
  useEffect(() => {
    const ai = window.electronAPI?.ai;
    let legacy: string | null = null;
    try {
      legacy = localStorage.getItem(LEGACY_KEY_STORAGE);
    } catch {
      legacy = null;
    }
    if (!ai) {
      if (legacy !== null) localStorage.removeItem(LEGACY_KEY_STORAGE);
      return;
    }
    const pending = legacy && legacy.trim() ? ai.setGeminiKey(legacy) : ai.credentialStatus();
    pending
      .then((status) => {
        if (legacy !== null) localStorage.removeItem(LEGACY_KEY_STORAGE);
        setCredentialStatus(status);
      })
      .catch((err) => {
        console.error('No se pudo consultar la clave de IA:', err instanceof Error ? err.message : err);
      });
  }, []);

  const handleSaveKey = async () => {
    const ai = window.electronAPI?.ai;
    if (!ai || !keyDraft.trim()) return;
    try {
      setCredentialStatus(await ai.setGeminiKey(keyDraft));
      setKeyDraft('');
    } catch (err) {
      setStatusMessage(`No se pudo guardar la clave: ${err instanceof Error ? err.message : String(err)}`);
    }
  };

  const handleClearKey = async () => {
    const ai = window.electronAPI?.ai;
    if (!ai) return;
    setCredentialStatus(await ai.clearGeminiKey());
  };

  useEffect(() => {
    historyEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [stepsHistory, currentStep, statusMessage]);

  const handleStart = async (e?: React.FormEvent) => {
    if (e) e.preventDefault();
    if (!goal.trim() || isRunning) return;

    if (mode === 'gemini' && !credentialStatus?.configured) {
      setShowSettings(true);
      setStatusMessage(window.electronAPI?.ai ? 'Por favor ingresa tu API Key de Gemini en los ajustes.' : 'El modo Gemini solo está disponible en la aplicación de escritorio.');
      return;
    }

    const controller = new AbortController();
    runControllerRef.current = controller;
    const { signal } = controller;

    setIsRunning(true);
    setCurrentStep(0);
    setFinalAnswer(null);
    setStepsHistory([]);
    setStatusMessage('Iniciando agente autónomo...');

    orchestratorRef.current?.reset();

    const maxSteps = 15;
    let stepCount = 0;
    let consecutiveFailures = 0;

    try {
      while (stepCount < maxSteps && !signal.aborted) {
        stepCount += 1;
        setCurrentStep(stepCount);
        setStatusMessage(`Paso ${stepCount}: Analizando página y decidiendo acción...`);

        // Pequeña pausa para permitir actualización de la UI
        await cancellableDelay(400, signal);

        const result = await orchestratorRef.current!.runStep(goal, mode, signal);

        setStepsHistory((prev) => [...prev, result]);

        if (result.finished) {
          setFinalAnswer(result.answer || 'Tarea completada exitosamente.');
          setStatusMessage('¡Objetivo completado!');
          break;
        }

        consecutiveFailures = result.failed ? consecutiveFailures + 1 : 0;
        if (shouldStopAfterFailures(consecutiveFailures)) {
          setStatusMessage(`Detenido: ${consecutiveFailures} pasos fallidos seguidos. Último error: ${result.execution_msg}`);
          break;
        }

        // Espera entre pasos para que el motor y la UI se estabilicen
        await cancellableDelay(1200, signal);
      }

      if (stepCount >= maxSteps && !signal.aborted) {
        setStatusMessage('Se alcanzó el límite máximo de pasos (15).');
      }
    } catch (err) {
      // Detener no es un error: `handleStop` ya puso su mensaje.
      if (!(err instanceof AgentCancelledError)) {
        console.error('Error durante la ejecución del agente:', err);
        setStatusMessage(`Error: ${err instanceof Error ? err.message : String(err)}`);
      }
    } finally {
      // Solo la ejecución vigente limpia el estado: si el usuario detuvo esta
      // y lanzó otra, esa otra sigue en marcha.
      if (runControllerRef.current === controller) {
        runControllerRef.current = null;
        setIsRunning(false);
      }
    }
  };

  const handleStop = () => {
    runControllerRef.current?.abort();
    runControllerRef.current = null;
    setIsRunning(false);
    setStatusMessage('Ejecución detenida por el usuario.');
  };

  const handleClear = () => {
    if (isRunning) return;
    setStepsHistory([]);
    setFinalAnswer(null);
    setCurrentStep(0);
    setStatusMessage('Listo para recibir instrucciones.');
    orchestratorRef.current?.reset();
  };

  if (!isOpen) return null;

  return (
    <aside className="agent-sidebar" aria-label="Panel del Agente IA">
      <div className="agent-header">
        <div className="agent-header-title">
          <span className="agent-badge">IA</span>
          <h3>Copiloto Navegador</h3>
        </div>
        <div className="agent-header-actions">
          <button
            onClick={() => setShowSettings(!showSettings)}
            className={`btn icon-btn ${showSettings ? 'active' : ''}`}
            title="Ajustes de IA"
            aria-label="Ajustes de IA"
          >
            ⚙️
          </button>
          <button
            onClick={onClose}
            className="btn icon-btn"
            title="Cerrar panel"
            aria-label="Cerrar panel"
          >
            ✕
          </button>
        </div>
      </div>

      {showSettings && (
        <div className="agent-settings-panel">
          <label className="settings-label">
            <span>Modo de Operación:</span>
            <select
              value={mode}
              onChange={(e) => setMode(e.target.value as 'simulation' | 'gemini')}
              className="settings-select"
            >
              <option value="simulation">⚡ Simulación Rápida (Sin API Key)</option>
              <option value="gemini">🧠 Gemini 2.0 Flash (Real AI)</option>
            </select>
          </label>

          {mode === 'gemini' && !window.electronAPI?.ai && (
            <small className="settings-hint">
              El modo Gemini solo está disponible en la aplicación de escritorio: la clave se guarda en su proceso principal, no en la página.
            </small>
          )}

          {mode === 'gemini' && window.electronAPI?.ai && (
            <label className="settings-label" style={{ marginTop: '8px' }}>
              <span>Gemini API Key:</span>
              <input
                type="password"
                value={keyDraft}
                onChange={(e) => setKeyDraft(e.target.value)}
                placeholder={credentialStatus?.configured ? 'Clave configurada — escribe otra para sustituirla' : 'AIzaSy...'}
                className="settings-input"
                autoComplete="off"
              />
              <div className="agent-controls" style={{ marginTop: '6px' }}>
                <button type="button" className="btn btn-primary" onClick={handleSaveKey} disabled={!keyDraft.trim()}>
                  Guardar clave
                </button>
                <button type="button" className="btn btn-secondary" onClick={handleClearKey} disabled={!credentialStatus?.configured}>
                  Borrar clave
                </button>
              </div>
              <small className="settings-hint">
                {!credentialStatus?.configured
                  ? 'Sin clave configurada.'
                  : credentialStatus.persisted
                    ? 'Guardada cifrada por el sistema operativo. La aplicación nunca la devuelve a esta página.'
                    : 'Este sistema no ofrece almacenamiento cifrado: la clave solo se conserva hasta cerrar la aplicación.'}
              </small>
            </label>
          )}
        </div>
      )}

      <div className="agent-status-bar">
        <div className={`status-indicator ${isRunning ? 'running' : 'idle'}`} />
        <span className="status-text">{statusMessage}</span>
      </div>

      <div className="agent-history-container">
        {stepsHistory.length === 0 && !finalAnswer && (
          <div className="agent-empty-state">
            <span className="empty-icon">🤖</span>
            <h4>¿Qué deseas investigar o buscar hoy?</h4>
            <p>
              Escribe una meta en lenguaje natural y el agente interactuará de forma autónoma con la web.
            </p>
            <div className="quick-prompts">
              <button
                className="quick-prompt-btn"
                onClick={() => setGoal('Busca en Wikipedia sobre la historia de Internet y resume el artículo')}
              >
                📖 Resumen de Wikipedia sobre Internet
              </button>
              <button
                className="quick-prompt-btn"
                onClick={() => setGoal('Busca noticias de Inteligencia Artificial en Google')}
              >
                🔍 Buscar noticias de IA en Google
              </button>
            </div>
          </div>
        )}

        {stepsHistory.map((step, idx) => (
          <div key={idx} className="step-card">
            <div className="step-header">
              <span className="step-number">Paso {idx + 1}</span>
              <span className={`step-action-tag action-${step.action}`}>{step.action}</span>
            </div>
            {step.thought && (
              <p className="step-thought">
                <strong>💭 Pensamiento:</strong> {step.thought}
              </p>
            )}
            {step.execution_msg && (
              <div className="step-execution">
                <strong>{step.failed ? '❌ Falló:' : '⚡ Acción:'}</strong> {step.execution_msg}
              </div>
            )}
          </div>
        ))}

        {finalAnswer && (
          <div className="final-answer-card">
            <div className="final-answer-header">
              <span>🎯 Respuesta del Agente</span>
            </div>
            <div className="final-answer-body">{finalAnswer}</div>
          </div>
        )}

        <div ref={historyEndRef} />
      </div>

      <form onSubmit={handleStart} className="agent-input-container">
        <textarea
          value={goal}
          onChange={(e) => setGoal(e.target.value)}
          placeholder="Ej: Navega a wikipedia y busca información sobre..."
          rows={2}
          disabled={isRunning}
          className="agent-textarea"
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              handleStart();
            }
          }}
        />
        <div className="agent-controls">
          <button
            type="button"
            onClick={handleClear}
            disabled={isRunning || (stepsHistory.length === 0 && !goal)}
            className="btn btn-secondary"
            title="Limpiar conversación"
          >
            Limpiar
          </button>
          {isRunning ? (
            <button
              type="button"
              onClick={handleStop}
              className="btn btn-danger"
            >
              ⏹ Detener
            </button>
          ) : (
            <button
              type="submit"
              disabled={!goal.trim()}
              className="btn btn-primary"
            >
              🚀 Iniciar Agente
            </button>
          )}
        </div>
      </form>
    </aside>
  );
};

export default AgentSidebar;
