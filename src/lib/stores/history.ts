import { writable } from 'svelte/store';
import type { CanvasState } from './canvas';

export interface NavigationHistory {
  stack: CanvasState[];
  canGoBack: boolean;
}

function createNavigationHistory() {
  const { subscribe, update } = writable<NavigationHistory>({
    stack: [],
    canGoBack: false
  });

  return {
    subscribe,
    pushState: (state: CanvasState) => {
      update(history => {
        const newStack = [...history.stack, state].slice(-20); // Keep only last 20 entries
        return {
          stack: newStack,
          canGoBack: newStack.length > 0
        };
      });
    },
    popState: (): CanvasState | null => {
      let popped: CanvasState | null = null;
      update(history => {
        if (history.stack.length > 0) {
          const newStack = [...history.stack];
          popped = newStack.pop()!;
          return {
            stack: newStack,
            canGoBack: newStack.length > 0
          };
        }
        return history;
      });
      return popped;
    },
    clear: () => {
      update(() => ({ stack: [], canGoBack: false }));
    }
  };
}

export const navigationHistory = createNavigationHistory();