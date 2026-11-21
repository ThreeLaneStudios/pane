import { invoke } from '@tauri-apps/api/core';
// OpenClaw workspace detection + agent discovery service
// v11.29.20 — AI-as-filesystem-bridge for universal agent discovery and file import

export interface WorkspaceFile {
  name: string;
  content: string;
  size: number;
  sensitiveLines: SensitiveLine[];
}

export interface SensitiveLine {
  lineNumber: number;
  line: string;
  pattern: string;
  redacted: boolean;
}

export interface AgentConfig {
  id: string;
  name: string;
  role: string;
  model: string;
  soul: string;
  memory: string;
  workspacePath: string;
}

export interface WorkspaceResult {
  files: WorkspaceFile[];
  needsRemoteImport: boolean;
}

const SENSITIVE_PATTERNS: { pattern: RegExp; label: string }[] = [
  { pattern: /sk-[a-zA-Z0-9]{20,}/g, label: 'OpenAI key' },
  { pattern: /sk-or-v1-[a-zA-Z0-9]{20,}/g, label: 'OpenRouter key' },
  { pattern: /ghp_[a-zA-Z0-9]{36,}/g, label: 'GitHub token' },
  { pattern: /eyJ[a-zA-Z0-9_-]{20,}/g, label: 'JWT token' },
  { pattern: /[a-f0-9]{40,}/g, label: 'API key (hex)' },
  { pattern: /password[:\s]*["']?[^\s"']{8,}/gi, label: 'Password' },
  { pattern: /\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b/g, label: 'IP address' },
];

export function detectSensitiveData(content: string): SensitiveLine[] {
  const lines = content.split('\n');
  const results: SensitiveLine[] = [];

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    for (const { pattern, label } of SENSITIVE_PATTERNS) {
      pattern.lastIndex = 0;
      if (pattern.test(line)) {
        results.push({
          lineNumber: i + 1,
          line,
          pattern: label,
          redacted: false,
        });
        break; // one flag per line
      }
    }
  }

  return results;
}

export function redactLine(content: string, lineNumber: number): string {
  const lines = content.split('\n');
  if (lineNumber > 0 && lineNumber <= lines.length) {
    let line = lines[lineNumber - 1];
    for (const { pattern } of SENSITIVE_PATTERNS) {
      pattern.lastIndex = 0;
      line = line.replace(pattern, '[REDACTED]');
    }
    lines[lineNumber - 1] = line;
  }
  return lines.join('\n');
}

function rawFilesToWorkspaceFiles(rawFiles: [string, string][]): WorkspaceFile[] {
  return rawFiles.map(([name, content]) => ({
    name,
    content,
    size: new TextEncoder().encode(content).length,
    sensitiveLines: detectSensitiveData(content),
  }));
}

/**
 * Fetch workspace files via direct filesystem read.
 * 1. Try default ~/.openclaw/workspace/
 * 2. If 0 files, return needsRemoteImport: true
 */
export async function fetchWorkspaceFiles(): Promise<WorkspaceResult> {
  try {
    const rawFiles: [string, string][] = await invoke('read_openclaw_workspace');
    console.log('[OC] Direct read found', rawFiles.length, 'files');
    if (rawFiles.length > 0) {
      return { files: rawFilesToWorkspaceFiles(rawFiles), needsRemoteImport: false };
    }
  } catch (e) {
    console.log('[OC] Direct read failed:', e);
  }

  return { files: [], needsRemoteImport: true };
}

/**
 * Fetch workspace files from a user-specified folder path.
 */
export async function fetchWorkspaceFilesFromPath(path: string): Promise<WorkspaceResult> {
  try {
    const rawFiles: [string, string][] = await invoke('read_workspace_from_path', { path });
    console.log('[OC] Custom path read found', rawFiles.length, 'files from', path);
    if (rawFiles.length > 0) {
      return { files: rawFilesToWorkspaceFiles(rawFiles), needsRemoteImport: false };
    }
  } catch (e) {
    console.log('[OC] Custom path read failed:', e);
  }

  return { files: [], needsRemoteImport: true };
}

/**
 * Trigger remote copy of workspace files via gateway chat completions.
 * Sends ONE request asking OC to cp workspace files to the shared folder,
 * then reads the copied files from the local filesystem.
 */
export async function triggerRemoteCopy(
  gatewayUrl: string,
  token: string,
  sharedPath: string,
  remotePath: string = '~/Shared/'
): Promise<WorkspaceResult> {
  const remoteTarget = remotePath.replace(/\/$/, '') + '/pane-workspace';
  const localTarget = sharedPath.replace(/\/$/, '') + '/pane-workspace';
  
  // Try reading from shared folder FIRST — files might already be there
  try {
    const rawFiles: [string, string][] = await invoke('read_workspace_from_path', { path: localTarget });
    console.log('[OC] Found', rawFiles.length, 'files already in shared folder');
    if (rawFiles.length > 0) {
      return { files: rawFilesToWorkspaceFiles(rawFiles), needsRemoteImport: false };
    }
  } catch (e) {
    console.log('[OC] No files in shared folder yet, will ask gateway to copy');
  }

  // Files not there yet — ask gateway to copy them
  const prompt = `Copy all .md files from ~/.openclaw/workspace/ to ${remoteTarget}/. Also copy ~/.openclaw/openclaw.json to ${remoteTarget}/. For any agents in openclaw.json that have custom workspace directories, copy their SOUL.md and MEMORY.md to ${remoteTarget}/agents/{agent-id}/. Use the exec tool or shell commands. Create directories as needed.`;

  const body = JSON.stringify({
    model: 'auto',
    messages: [{ role: 'user', content: prompt }],
  });

  // Destination and bearer token are resolved in Rust from the paired Gateway config.
  // Keep the public parameters for compatibility with existing callers, but never
  // forward renderer-controlled network authority to the backend.
  void gatewayUrl;
  void token;

  try {
    await invoke('gateway_chat_completions', { body });
    console.log('[OC] Remote copy request completed');
  } catch (e) {
    console.error('[OC] Remote copy request failed:', e);
    // Don't throw yet — try reading anyway in case files appeared via sync
  }

  // Read from shared folder after copy attempt
  try {
    const rawFiles: [string, string][] = await invoke('read_workspace_from_path', { path: localTarget });
    console.log('[OC] Read', rawFiles.length, 'files from shared folder');
    if (rawFiles.length > 0) {
      return { files: rawFilesToWorkspaceFiles(rawFiles), needsRemoteImport: false };
    }
  } catch (e) {
    console.log('[OC] Failed to read from shared folder:', e);
  }

  throw new Error('No workspace files found in the shared folder. Make sure the folder is synced between both machines, or copy the files manually.');
}

/**
 * Fetch agent configs by reading openclaw.json directly from filesystem.
 * If workspacePath is provided, looks for openclaw.json in that directory.
 */
// Sampling params for gatewayAIRequest — deterministic gateway probe, no creative variance needed.
// Values live here at the call-policy layer; the provider layer reads from the request struct.
const GATEWAY_AI_REQUEST_MAX_TOKENS = 16000;
// GATEWAY_AI_REQUEST_TEMPERATURE removed (#1846): temperature is not sent on the OC gateway path.
// OC manages its own downstream model; Pane should not inject temperature it cannot gate properly.
// Provider default is acceptable for structured JSON probes.

/**
 * Send a structured prompt to the OpenClaw gateway and extract JSON from the response.
 * Uses the AI as a filesystem bridge — works for any setup (local, Tailscale, VPS).
 */
export async function gatewayAIRequest(
  gatewayUrl: string,
  token: string,
  prompt: string,
  model: string = 'auto'
): Promise<string> {
  const body = JSON.stringify({
    model,
    messages: [{ role: 'user', content: prompt }],
    max_tokens: GATEWAY_AI_REQUEST_MAX_TOKENS,
  });

  // Rust derives the exact paired Gateway URL and token. The renderer supplies
  // only the OpenAI-compatible request body.
  void gatewayUrl;
  void token;
  const response: string = await invoke('gateway_chat_completions', { body });

  // Parse the chat completion response
  const parsed = JSON.parse(response);
  const content = parsed.choices?.[0]?.message?.content || '';

  // Extract JSON from the response (AI might wrap it in markdown code blocks)
  const jsonMatch =
    content.match(/```(?:json)?\s*([\s\S]*?)```/) || content.match(/(\{[\s\S]*\})/);
  if (jsonMatch) {
    return jsonMatch[1].trim();
  }
  return content.trim();
}

export async function fetchAgentConfigs(workspacePath?: string): Promise<AgentConfig[]> {
  try {
    let configPath: string | undefined;
    if (workspacePath) {
      // If workspacePath is a pane-workspace dir, look for openclaw.json there
      configPath = workspacePath.replace(/\/$/, '') + '/openclaw.json';
    }

    const raw: string = await invoke('read_openclaw_agents', { configPath: configPath || null });
    console.log('[OC] Direct agent read, length:', raw.length);
    const agentsObj = JSON.parse(raw);
    const agents: AgentConfig[] = [];
    for (const [id, cfg] of Object.entries(agentsObj)) {
      const c = cfg as any;
      agents.push({
        id,
        name: c.display_name || c.name || (c.identity && c.identity.name) || id,
        role: c.role || c.description || (c.identity && c.identity.emoji ? c.identity.emoji + ' Agent' : 'Agent'),
        model: c.model_display || (typeof c.model === 'string' ? c.model : (c.model && c.model.primary) || 'unknown'),
        soul: c.soul || '',
        memory: c.memory || '',
        workspacePath: c.workspace || '',
      });
    }
    return agents;
  } catch (e) {
    console.log('[OC] Agent read failed:', e);
    return [];
  }
}
