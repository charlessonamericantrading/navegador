export {};

export interface EngineStateEvent {
  type: 'state' | 'ready';
  tab_id?: number;
  scroll_offset_y?: number;
  url?: string;
  title?: string;
  screenshot?: string;
  elements?: any[];
  can_go_back?: boolean;
  can_go_forward?: boolean;
}

declare global {
  interface BackendStatusEvent {
    status: 'restarting' | 'failed';
    attempt?: number;
    message?: string;
  }

  interface UpdateStatusEvent {
    status: 'available' | 'downloaded';
  }

  interface Window {
    electronAPI?: {
      isElectron: boolean;
      platform: string;
      sendEngineRequest: (payload: any) => Promise<any>;
      onEngineState: (callback: (data: EngineStateEvent) => void) => () => void;
      openExternal: (url: string) => void;
      onBackendStatus: (callback: (data: BackendStatusEvent) => void) => () => void;
      onUpdateStatus: (callback: (data: UpdateStatusEvent) => void) => () => void;
      installUpdate: () => void;
    };
  }
}
