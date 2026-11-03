import type { ChatRecordKind } from './chat_contract.ts';

export const SEARCH_LIMITS = { query_bytes: 256, page_hits: 25, page_bytes: 4 * 1024 * 1024, page_records: 256,
  preview_characters: 80, response_bytes: 256 * 1024, title_bytes: 512, display_hits: 250 } as const;
export type ConversationKind = 'chat' | 'workspace';
export type SearchScope = 'all' | 'saved' | 'workspace';
export interface SearchRequest {
  query: string; case_sensitive: boolean; scope: SearchScope; include_archived: boolean; limit: number; cursor: SearchCursor | null;
}
export interface SearchCursor {
  schema_version: number; query: string; case_sensitive: boolean; scope: SearchScope; include_archived: boolean;
  kind: ConversationKind; after_chat_id: string | null; chat_id: string | null; generation: number; version: number;
  record_sequence_cutoff: number; record_count: number; records_seen: number; after_sequence: number; title_done: boolean;
}
export interface SearchPreview {
  before: string; matched: string; after: string; match_count: number; byte_offset: number; end_byte_offset: number; truncated: boolean;
}
export type SearchTarget = { target: 'title' }
  | { target: 'record'; record_id: string; sequence: number; record_kind: ChatRecordKind; correlation_id: string };
export interface SearchHit {
  kind: ConversationKind; chat_id: string; generation: number; version: number; title: string | null;
  location: SearchTarget; preview: SearchPreview;
}
export interface SearchStats {
  chats_scanned: number; records_scanned: number; bytes_scanned: number; unavailable_records: number; unavailable_titles: number; purged_chats: number; changed_chats: number;
}
export interface SearchPage {
  schema_version: number; hits: SearchHit[]; stats: SearchStats; next_cursor: SearchCursor | null; complete: boolean;
}
export interface SearchBackend {
  chat_search_request(project_key: string, request: SearchRequest): Promise<SearchPage>;
}
