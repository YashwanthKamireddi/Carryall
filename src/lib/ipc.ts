/** Typed wrappers around the Rust IPC commands and events. */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ContentKind =
  | "text"
  | "link"
  | "color"
  | "code"
  | "rich_text"
  | "image"
  | "files"
  | "video"
  | "audio"
  | "binary";

export interface Clip {
  id: string;
  kind: ContentKind;
  preview: string;
  size: number;
  created_at: number;
  pinned: boolean;
  thumb?: string;
}

export const listClips = (limit?: number) =>
  invoke<Clip[]>("list_clips", { limit });

export const searchClips = (query: string, limit?: number) =>
  invoke<Clip[]>("search_clips", { query, limit });

export const copyClip = (id: string) => invoke<void>("copy_clip", { id });

export const setPin = (id: string, pinned: boolean) =>
  invoke<void>("set_pin", { id, pinned });

export const deleteClip = (id: string) => invoke<void>("delete_clip", { id });

export const clearHistory = () => invoke<void>("clear_history");

export const hidePalette = () => invoke<void>("hide_palette");

/** Subscribe to live "a new clip was captured" pushes from the backend. */
export const onNewClip = (cb: (clip: Clip) => void): Promise<UnlistenFn> =>
  listen<Clip>("clip:new", (e) => cb(e.payload));
