import { writable } from 'svelte/store';

/**
 * Refresh trigger store
 * Increment this to trigger reactive updates across the app
 */
export const refreshTrigger = writable(0);

/**
 * Trigger a refresh of notes/tasks/data views
 */
export function triggerRefresh() {
  refreshTrigger.update(n => n + 1);
}
