/// <reference types="vite/client" />

interface MonacoEnvironmentConfig {
  getWorker(workerId: string, label: string): Worker;
}

interface Window {
  MonacoEnvironment?: MonacoEnvironmentConfig;
}
