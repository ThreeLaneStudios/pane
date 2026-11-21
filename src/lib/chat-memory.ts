import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

const COMPACTION_THRESHOLD = 50;
const KEEP_LAST = 20;

const SUMMARY_PROMPT = `You are a conversation summarizer. Summarize the following conversation messages into a concise memory document. Include: key decisions made, important context, action items, preferences discussed, and current status of any work. Keep it under 400 words. Write in third person ("The user asked about...", "It was decided that..."). Do NOT include greetings or meta-commentary.`;

export async function getChatMessageCount(threadId: string): Promise<number> {
  return await invoke('get_chat_message_count', { threadId }) as number;
}

export async function getMemoryContext(projectId: string | undefined): Promise<string | null> {
  const memory = await invoke('get_chat_memory', { projectId: projectId || null }) as string | null;
  return memory || null;
}

export async function saveChatMemory(projectId: string | undefined, content: string): Promise<void> {
  await invoke('save_chat_memory', { projectId: projectId || null, content });
}

export async function deleteOldMessages(threadId: string, keepLast: number = KEEP_LAST): Promise<void> {
  await invoke('delete_old_chat_messages', { threadId, keepLast });
}

/**
 * Check if compaction is needed. Returns true if message count exceeds threshold.
 */
export async function shouldCompact(threadId: string): Promise<boolean> {
  const count = await getChatMessageCount(threadId);
  console.log(`[COMPACTION] Message count for thread ${threadId}: ${count}, threshold: ${COMPACTION_THRESHOLD}`);
  return count >= COMPACTION_THRESHOLD;
}

/**
 * Run compaction: summarize old messages, save summary, delete old messages.
 * Returns the summary text on success, null on failure.
 */
export async function runCompaction(
  threadId: string,
  projectId: string | undefined,
  allMessages: Array<{ role: string; content: string }>,
): Promise<string | null> {
  const messagesToSummarize = allMessages.slice(0, Math.max(0, allMessages.length - KEEP_LAST));
  
  if (messagesToSummarize.length < 5) {
    console.log('[COMPACTION] Not enough old messages to summarize');
    return null;
  }

  console.log(`[COMPACTION] Summarizing ${messagesToSummarize.length} old messages, keeping last ${KEEP_LAST}`);

  const conversationText = messagesToSummarize
    .filter(m => m.role === 'user' || m.role === 'assistant')
    .map(m => `${m.role === 'user' ? 'User' : 'Assistant'}: ${m.content}`)
    .join('\n\n');

  const summaryMessages = [
    { role: 'user', content: `${SUMMARY_PROMPT}\n\n---\n\n${conversationText}` }
  ];

  return new Promise<string | null>(async (resolve) => {
    let summaryText = '';
    let unlisten: (() => void) | null = null;
    let resolved = false;
    
    try {
      const streamId = await invoke('ai_chat_stream', { 
        messages: summaryMessages, 
        context: { projectId: projectId || undefined, compaction: true } 
      }) as string;

      unlisten = await listen('ai-stream-chunk', (event: any) => {
        const data = event.payload;
        if (data.id !== streamId) return;
        
        if (data.done) {
          summaryText = data.fullText || summaryText;
          console.log(`[COMPACTION] Summary generated: ${summaryText.length} chars`);
          
          if (unlisten) unlisten();
          
          // Safety guard: never delete messages if summary is empty or too short
          if (!summaryText || summaryText.trim().length < 50) {
            console.warn("[COMPACTION] Summary too short or empty — aborting compaction. Messages preserved.");
            resolved = true;
            resolve(null);
            return;
          }
          
          (async () => {
            try {
              await saveChatMemory(projectId, summaryText);
              await deleteOldMessages(threadId, KEEP_LAST);
              console.log('[COMPACTION] Memory saved and old messages deleted');
              resolved = true;
              resolve(summaryText);
            } catch (e) {
              console.error('[COMPACTION] Failed to save/delete:', e);
              resolved = true;
              resolve(null);
            }
          })();
        } else if (data.text) {
          summaryText += data.text;
        } else if (data.error) {
          console.error('[COMPACTION] Stream error:', data.error);
          if (unlisten) unlisten();
          resolved = true;
          resolve(null);
        }
      }) as unknown as () => void;

      setTimeout(() => {
        if (!resolved) {
          if (unlisten) unlisten();
          console.error('[COMPACTION] Timed out');
          resolve(null);
        }
      }, 30000);
      
    } catch (e) {
      console.error('[COMPACTION] Failed to start stream:', e);
      if (unlisten) unlisten();
      resolve(null);
    }
  });
}
