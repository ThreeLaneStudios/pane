/**
 * Format a model ID into a short, human-readable name.
 * Used for displaying model labels in the chat UI.
 * 
 * @param modelId - Full model ID (e.g., "claude-sonnet-4-20250514" or "anthropic/claude-opus-4")
 * @returns Shortened model name (e.g., "sonnet-4")
 */
export function formatModelName(modelId: string | null | undefined): string {
  if (!modelId) return '';
  
  // Strip provider prefix if present (e.g., "anthropic/claude-opus-4" -> "claude-opus-4")
  const withoutProvider = modelId.includes('/') 
    ? modelId.split('/').pop() || modelId 
    : modelId;
  
  // Mapping of full model IDs to short names
  const shortcuts: Record<string, string> = {
    'claude-3-haiku-20240307': 'haiku-3',
    'claude-3-5-haiku-20241022': 'haiku-3.5',
    'claude-3-5-sonnet-20241022': 'sonnet-3.5',
    'claude-sonnet-4-20250514': 'sonnet-4',
    'claude-opus-4-20250514': 'opus-4',
    'claude-opus-4-5': 'opus-4.5',
    'claude-sonnet-4-5': 'sonnet-4.5',
    'gpt-4o': 'gpt-4o',
    'gpt-4o-mini': 'gpt-4o-mini',
    'gpt-4-turbo': 'gpt-4-turbo',
    'gpt-3.5-turbo': 'gpt-3.5-turbo',
    'gemini-2.5-pro': 'Gemini 2.5 Pro',
  };
  
  // Return shortcut if it exists, otherwise return the model ID without provider
  return shortcuts[withoutProvider] || withoutProvider;
}
