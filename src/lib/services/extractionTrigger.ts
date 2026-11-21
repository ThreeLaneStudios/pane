/**
 * Extraction trigger logic — extracted from ChatSidebar for testability.
 *
 * Determines whether a chat exchange should trigger memory extraction
 * based on message word count and structural tool usage.
 */

/** Tools that modify workspace structure (create/edit/delete notes, projects, etc.) */
export const STRUCTURAL_TOOLS = [
  'create_note',
  'create_folder',
  'create_project',
  'edit_note',
  'set_status',
  'move_note',
  'delete_note',
  'delete_folder',
] as const;

/** Minimum word count for a user message to trigger extraction on its own */
export const MIN_WORD_COUNT = 8;

export interface ToolCall {
  name: string;
  [key: string]: unknown;
}

/**
 * Determine whether memory extraction should run for a given exchange.
 *
 * @param userMessage - The user's message text
 * @param toolCalls  - Tool calls the AI made in its response
 * @returns true if extraction should be triggered
 */
export function shouldExtractMemory(
  userMessage: string,
  toolCalls: ToolCall[] = [],
): boolean {
  const wordCount = userMessage
    .split(/\s+/)
    .filter(Boolean).length;

  const aiUsedStructuralTool = toolCalls.some((tc) =>
    (STRUCTURAL_TOOLS as readonly string[]).includes(tc.name),
  );

  const userMessageLongEnough = wordCount >= MIN_WORD_COUNT;

  return aiUsedStructuralTool || userMessageLongEnough;
}
