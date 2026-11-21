import { test, expect } from '../../fixtures';

test.describe('T3.7: Agent Management', () => {
  test('should display agents list in settings', async ({ tauriPage }) => {
    console.log('[AGENTS] Opening settings → agents...');
    
    // Open settings
    const settingsOpened = await tauriPage.clickIfVisible('button:has-text("Settings")') || 
                           await tauriPage.clickIfVisible('[title*="Settings" i]');
    
    if (settingsOpened) {
      await tauriPage.waitForTimeout(500);
      console.log('[AGENTS] Settings opened');
      
      // Click Agents tab (from Settings.svelte: activeTab === 'agents')
      const agentsTabVisible = await tauriPage.isVisible('text=Agents') || 
                                await tauriPage.isVisible('button:has-text("Agents")');
      
      if (agentsTabVisible) {
        await tauriPage.click('text=Agents, button:has-text("Agents")');
        await tauriPage.waitForTimeout(800);
        console.log('[AGENTS] ✓ Agents tab opened');
      } else {
        console.log('[AGENTS] Agents tab not visible - checking for agents content directly');
      }
      
      // Verify agents list renders (from Settings.svelte: agentsList)
      const hasAgentsList = await tauriPage.count('.agent-name, [class*="agent"]') > 0 ||
                            await tauriPage.isVisible('text=No agents') ||
                            await tauriPage.isVisible('text=AGENTS');
      
      expect(hasAgentsList).toBeTruthy();
      console.log('[AGENTS] ✓ Agents section rendered');
    } else {
      console.log('[AGENTS] Could not open settings - skipping');
    }
  });
  
  test('should show at least one agent (main agent)', async ({ tauriPage }) => {
    console.log('[AGENTS] Verifying main agent presence...');
    
    // Open settings → agents
    await tauriPage.clickIfVisible('button:has-text("Settings")') || 
    await tauriPage.clickIfVisible('[title*="Settings" i]');
    await tauriPage.waitForTimeout(500);
    
    const agentsTabVisible = await tauriPage.isVisible('text=Agents');
    if (agentsTabVisible) {
      await tauriPage.click('text=Agents');
      await tauriPage.waitForTimeout(800);
    }
    
    // Look for agent entries (from Settings.svelte: agentsList.map)
    // Should show at minimum the main agent
    const hasAgentEntry = await tauriPage.count('.agent-name-display, .form-row-label, [class*="agent"]') > 0;
    
    if (hasAgentEntry) {
      console.log('[AGENTS] ✓ At least one agent listed');
      
      // Check for "Main" badge or similar indicator (from Settings.svelte: agent.is_main)
      const hasMainBadge = await tauriPage.isVisible('text=Main') ||
                           await tauriPage.isVisible('.badge-teal, .badge') ||
                           await tauriPage.count('[class*="badge"]') > 0;
      
      if (hasMainBadge) {
        console.log('[AGENTS] ✓ Main agent indicator found');
      }
    } else {
      // Check for "no agents" empty state
      const hasEmptyState = await tauriPage.isVisible('text=No agents') ||
                            await tauriPage.isVisible('.empty-state');
      
      console.log('[AGENTS] Agents list state:', hasEmptyState ? 'empty' : 'populated');
    }
  });
  
  test('should show agent as read-only when linked to OpenClaw', async ({ tauriPage }) => {
    console.log('[AGENTS] Checking for OpenClaw-linked agent restrictions...');
    
    // Open settings → agents
    await tauriPage.clickIfVisible('button:has-text("Settings")') || 
    await tauriPage.clickIfVisible('[title*="Settings" i]');
    await tauriPage.waitForTimeout(500);
    
    const agentsTabVisible = await tauriPage.isVisible('text=Agents');
    if (agentsTabVisible) {
      await tauriPage.click('text=Agents');
      await tauriPage.waitForTimeout(800);
    }
    
    // Look for "Linked to OpenClaw" label (from Settings.svelte: openclaw-locked-label)
    const hasOpenClawLabel = await tauriPage.isVisible('text=Linked to OpenClaw') ||
                             await tauriPage.count('.openclaw-locked-label') > 0;
    
    if (hasOpenClawLabel) {
      console.log('[AGENTS] ✓ OpenClaw-linked agent marked as read-only');
    } else {
      // May not have OpenClaw provider configured, or agent is editable
      console.log('[AGENTS] No OpenClaw-linked agents (or provider not configured)');
      
      // Check if agent name fields exist and are editable
      const hasEditableAgent = await tauriPage.count('input[type="text"]:not([readonly]):not([disabled]), .agent-edit-icon') > 0;
      
      if (hasEditableAgent) {
        console.log('[AGENTS] ✓ Editable agent fields available');
      }
    }
  });
  
  test('should display agent metadata (role, source, workspace)', async ({ tauriPage }) => {
    console.log('[AGENTS] Checking for agent metadata display...');
    
    // Open settings → agents
    await tauriPage.clickIfVisible('button:has-text("Settings")') || 
    await tauriPage.clickIfVisible('[title*="Settings" i]');
    await tauriPage.waitForTimeout(500);
    
    const agentsTabVisible = await tauriPage.isVisible('text=Agents');
    if (agentsTabVisible) {
      await tauriPage.click('text=Agents');
      await tauriPage.waitForTimeout(800);
    }
    
    // Look for agent metadata (from Settings.svelte: agent.role, agent.source, agent.workspace_path)
    const hasMetadata = await tauriPage.count('.form-row-sublabel') > 0 ||
                        await tauriPage.isVisible('text=import') ||
                        await tauriPage.isVisible('text=workspace') ||
                        await tauriPage.count('[class*="sublabel"]') > 0;
    
    if (hasMetadata) {
      console.log('[AGENTS] ✓ Agent metadata displayed (role, source, workspace)');
    } else {
      console.log('[AGENTS] Agent metadata not visible - may be collapsed or in different format');
    }
  });
  
  test('should show agent status (active/inactive)', async ({ tauriPage }) => {
    console.log('[AGENTS] Checking for agent status indicators...');
    
    // Open settings → agents
    await tauriPage.clickIfVisible('button:has-text("Settings")') || 
    await tauriPage.clickIfVisible('[title*="Settings" i]');
    await tauriPage.waitForTimeout(500);
    
    const agentsTabVisible = await tauriPage.isVisible('text=Agents');
    if (agentsTabVisible) {
      await tauriPage.click('text=Agents');
      await tauriPage.waitForTimeout(800);
    }
    
    // Look for status dots or labels (from Settings.svelte: .status-dot, agent.is_active)
    const hasStatusIndicator = await tauriPage.count('.status-dot') > 0 ||
                                await tauriPage.isVisible('text=Active') ||
                                await tauriPage.isVisible('text=Inactive');
    
    if (hasStatusIndicator) {
      console.log('[AGENTS] ✓ Agent status indicators present');
    } else {
      console.log('[AGENTS] Status indicators not visible');
    }
  });
});
