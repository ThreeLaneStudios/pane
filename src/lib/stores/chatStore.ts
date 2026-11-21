import { writable } from 'svelte/store';
import type { ChatMessage } from '$lib/types';

// Global chat messages store - persists across component remounts
export const chatMessages = writable<ChatMessage[]>([]);

// Helper to add a message
export function addChatMessage(message: ChatMessage) {
  chatMessages.update(messages => [...messages, message]);
}

// Helper to clear all messages
export function clearChatMessages() {
  chatMessages.set([]);
}

// Helper to replace messages (e.g., after loading from DB)
export function setChatMessages(messages: ChatMessage[]) {
  chatMessages.set(messages);
}

// Helper to update a specific message
export function updateChatMessage(id: string, updater: (msg: ChatMessage) => ChatMessage) {
  chatMessages.update(messages => 
    messages.map(m => m.id === id ? updater(m) : m)
  );
}

// Helper to remove a message
export function removeChatMessage(id: string) {
  chatMessages.update(messages => messages.filter(m => m.id !== id));
}
