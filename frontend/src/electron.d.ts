import type { InteractiveElement } from './domains/agent/AgentOrchestrator';

// Respuestas del motor tal como las serializa `EngineResponse` en
// `engine/crates/core/src/protocol.rs`. Union discriminada por `type`: tras
// comprobar `res.type === 'error'`, TypeScript sabe que hay `message`.

export interface EngineStateEvent {
  type: 'state' | 'ready';
  /** `null`/ausente en una publicacion espontanea del motor (Fase 50). */
  id?: string | null;
  tab_id?: number;
  scroll_offset_y?: number;
  url?: string;
  title?: string;
  screenshot?: string;
  elements?: InteractiveElement[];
  requires_javascript?: boolean;
  can_go_back?: boolean;
  can_go_forward?: boolean;
}

export interface EngineTabsResponse {
  type: 'tabs';
  id?: string | null;
  tabs: { id: number; title: string; url: string }[];
  active_tab_id: number;
}

export interface EngineErrorResponse {
  type: 'error';
  id?: string | null;
  message: string;
}

export interface EngineOtherResponse {
  type: 'pong' | 'ok' | 'accessibility_tree';
  id?: string | null;
}

export type EngineResponse = EngineStateEvent | EngineTabsResponse | EngineErrorResponse | EngineOtherResponse;

/** Lo que valida `desktop/engine-protocol.js`; aqui basta con la forma. */
export type EngineRequestPayload = { type: string } & Record<string, unknown>;

declare global {
  interface BackendStatusEvent {
    status: 'restarting' | 'failed';
    attempt?: number;
    message?: string;
  }

  interface UpdateStatusEvent {
    status: 'available' | 'downloaded';
  }

  /** Lo único que el renderer sabe de la clave de IA (plan H04). */
  interface AiCredentialStatus {
    configured: boolean;
    /** Cifrada en disco. `false` con `configured` = solo para esta sesión. */
    persisted: boolean;
    secureStorage: boolean;
  }

  interface Window {
    electronAPI?: {
      isElectron: boolean;
      platform: string;
      sendEngineRequest: (payload: EngineRequestPayload) => Promise<EngineResponse>;
      onEngineState: (callback: (data: EngineStateEvent) => void) => () => void;
      openExternal: (url: string) => void;
      onBackendStatus: (callback: (data: BackendStatusEvent) => void) => () => void;
      onUpdateStatus: (callback: (data: UpdateStatusEvent) => void) => () => void;
      installUpdate: () => void;
      ai: {
        credentialStatus: () => Promise<AiCredentialStatus>;
        setGeminiKey: (key: string) => Promise<AiCredentialStatus>;
        clearGeminiKey: () => Promise<AiCredentialStatus>;
        generate: (requestId: string, prompt: string) => Promise<string>;
        cancel: (requestId: string) => Promise<void>;
      };
    };
  }
}
