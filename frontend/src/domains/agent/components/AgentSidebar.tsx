import React, { useState, useEffect, useRef } from 'react';
import { AgentOrchestrator, type AgentStepResult, type BrowserInterface } from '../AgentOrchestrator';

interface AgentSidebarProps {
  isOpen: boolean;
  onClose: () => void;
  browserInterface: BrowserInterface;
}

export const AgentSidebar: React.FC<AgentSidebarProps> = ({ isOpen, onClose, browserInterface }) => {
  const [goal, setGoal] = useState('');
  const [mode, setMode] = useState<'simulation' | 'gemini'>('simulation');
  const [apiKey, setApiKey] = useState(() => localStorage.getItem('gemini_api_key') || '');
  const [showSettings, setShowSettings] = useState(false);
  const [isRunning, setIsRunning] = useState(false);
  const [currentStep, setCurrentStep] = useState(0);
  const [statusMessage, setStatusMessage] = useState<string>('Listo para recibir instrucciones.');
  const [stepsHistory, setStepsHistory] = useState<AgentStepResult[]>([]);
  const [finalAnswer, setFinalAnswer] = useState<string | null>(null);

  const isRunningRef = useRef(false);
  const orchestratorRef = useRef<AgentOrchestrator | null>(null);
  const historyEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    orchestratorRef.current = new AgentOrchestrator(browserInterface);
  }, [browserInterface]);

  useEffect(() => {
    localStorage.setItem('gemini_api_key', apiKey);
  }, [apiKey]);

  useEffect(() => {
    historyEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [stepsHistory, currentStep, statusMessage]);

  const handleStart = async (e?: React.FormEvent) => {
    if (e) e.preventDefault();
    if (!goal.trim() || isRunning) return;

    if (mode === 'gemini' && !apiKey.trim()) {
      setShowSettings(true);
      setStatusMessage('Por favor ingresa tu API Key de Gemini en los ajustes.');
      return;
    }

    setIsRunning(true);
    isRunningRef.current = true;
    setCurrentStep(0);
    setFinalAnswer(null);
    setStepsHistory([]);
    setStatusMessage('Iniciando agente autónomo...');

    orchestratorRef.current?.reset();

    const maxSteps = 15;
    let stepCount = 0;

    try {
      while (stepCount < maxSteps && isRunningRef.current) {
        stepCount += 1;
        setCurrentStep(stepCount);
        setStatusMessage(`Paso ${stepCount}: Analizando página y decidiendo acción...`);

        // Pequeña pausa para permitir actualización de la UI
        await new Promise((r) => setTimeout(r, 400));
        if (!isRunningRef.current) break;

        const result = await orchestratorRef.current!.runStep(
          goal,
          mode,
          mode === 'gemini' ? apiKey : undefined
        );

        setStepsHistory((prev) => [...prev, result]);

        if (result.finished) {
          setFinalAnswer(result.answer || 'Tarea completada exitosamente.');
          setStatusMessage('¡Objetivo completado!');
          break;
        }

        // Espera entre pasos para que el motor y la UI se estabilicen
        await new Promise((r) => setTimeout(r, 1200));
      }

      if (stepCount >= maxSteps && isRunningRef.current) {
        setStatusMessage('Se alcanzó el límite máximo de pasos (15).');
      }
    } catch (err: any) {
      console.error('Error durante la ejecución del agente:', err);
      setStatusMessage(`Error: ${err.message || err}`);
    } finally {
      setIsRunning(false);
      isRunningRef.current = false;
    }
  };

  const handleStop = () => {
    isRunningRef.current = false;
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

          {mode === 'gemini' && (
            <label className="settings-label" style={{ marginTop: '8px' }}>
              <span>Gemini API Key:</span>
              <input
                type="password"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder="AIzaSy..."
                className="settings-input"
              />
              <small className="settings-hint">
                Se guarda únicamente en el almacenamiento local de tu navegador.
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
                <strong>⚡ Acción:</strong> {step.execution_msg}
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
