export const EXPORT_LIMITS = { records: 4096 } as const;
export type ExportKind = 'chat' | 'workspace';
export type ExportFormat = 'json' | 'text';

/** Native code owns the save dialog, destination, and private content reads. */
export interface ExportRequest {
  kind: ExportKind; format: ExportFormat; chat_id: string; generation: number; expected_version: number;
}
export interface ExportReceipt {
  status: 'saved' | 'cancelled'; kind: ExportKind; format: ExportFormat; chat_id: string; generation: number;
  version: number; record_count: number; byte_count: number; partial: boolean;
}
export interface ExportBackend {
  chat_export_request(project_key: string, request: ExportRequest): Promise<ExportReceipt>;
}
/** The parent derives enabled from the selected canonical, quiescent state. */
export interface ExportSelection {
  project_key: string; kind: ExportKind; chat_id: string; generation: number; version: number; enabled: boolean;
}
