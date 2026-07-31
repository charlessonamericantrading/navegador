export {};

declare global {
  interface BackendStatusEvent {
    status: 'restarting' | 'failed';
    attempt?: number;
  }

  interface UpdateStatusEvent {
    status: 'available' | 'downloaded';
  }

  interface Window {
    electronAPI?: {
      isElectron: boolean;
      platform: string;
      openExternal: (url: string) => void;
      onBackendStatus: (callback: (data: BackendStatusEvent) => void) => () => void;
      onUpdateStatus: (callback: (data: UpdateStatusEvent) => void) => () => void;
      installUpdate: () => void;
    };
  }
}
