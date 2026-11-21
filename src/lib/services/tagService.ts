// Pane v9.0.0 — Tag Service

import { invoke } from '@tauri-apps/api/core';

// === Types ===

export interface TagInfo {
  id: number;
  name: string;
  color: string;
}

export interface CreateTagRequest {
  name: string;
  color?: string;
}

export interface UpdateTagRequest {
  tag_id: number;
  name?: string;
  color?: string;
}

export interface AddNoteTagsRequest {
  note_id: string;
  tag_ids: number[];
}

export interface RemoveNoteTagsRequest {
  note_id: string;
  tag_ids: number[];
}

// === Tag Operations ===

export async function getTags(): Promise<TagInfo[]> {
  return await invoke('get_tags');
}

export async function createTag(request: CreateTagRequest): Promise<TagInfo> {
  return await invoke('create_tag', { request });
}

export async function updateTag(request: UpdateTagRequest): Promise<TagInfo> {
  return await invoke('update_tag', { request });
}

export async function deleteTag(tagId: number): Promise<void> {
  await invoke('delete_tag', { tagId });
}

export async function addNoteTags(request: AddNoteTagsRequest): Promise<void> {
  await invoke('add_note_tags', { request });
}

export async function removeNoteTags(request: RemoveNoteTagsRequest): Promise<void> {
  await invoke('remove_note_tags', { request });
}

export async function getNotesByTag(tagId: number): Promise<string[]> {
  return await invoke('get_notes_by_tag', { tagId });
}

// === Helper Functions ===

export function getTagByName(tags: TagInfo[], name: string): TagInfo | undefined {
  return tags.find(tag => tag.name.toLowerCase() === name.toLowerCase());
}

export function createTagIfNotExists(
  tags: TagInfo[],
  name: string,
  color?: string
): Promise<TagInfo> {
  const existing = getTagByName(tags, name);
  if (existing) {
    return Promise.resolve(existing);
  }
  return createTag({ name, color });
}
