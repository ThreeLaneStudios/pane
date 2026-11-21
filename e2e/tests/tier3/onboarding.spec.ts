import { test, expect } from '../../fixtures';

test.describe('T3.2: Onboarding Flow', () => {
  // SKIP: This test requires a fresh database state (no onboarding_completed flag).
  // (The former __PANE_TEST_MODE__ auth bypass is gone — #3195 / ADR-0070 — so
  // this is now purely a DB-reset problem.) Marking as placeholder until we
  // implement a DB reset mechanism or manual onboarding trigger for E2E tests.
  test.skip('should complete onboarding wizard (PLACEHOLDER)', async ({ tauriPage }) => {
    console.log('[ONBOARDING] SKIPPED - requires fresh DB setup');
    console.log('[ONBOARDING] TODO: Implement DB reset mechanism for E2E');
    console.log('[ONBOARDING] TODO: Test each onboarding step, field validation, completion flow');
    
    // Expected steps (from spec):
    // 1. Verify each onboarding step renders
    // 2. Fill required fields (name, workspace, preferences, etc.)
    // 3. Complete wizard (next/finish buttons)
    // 4. Verify main UI loads after completion
    // 5. Verify onboarding_completed flag set in DB
    
    // Placeholder test - always passes
    expect(true).toBeTruthy();
  });
  
  // Alternative: Test that onboarding does NOT show for existing users
  test('should skip onboarding for existing setup', async ({ tauriPage }) => {
    console.log('[ONBOARDING] Verifying app loads directly (no onboarding)...');
    
    // Wait for main app UI to render (means onboarding was skipped or already complete)
    const appLoadedIndicator = await tauriPage.waitForSelector('[data-testid="app-loaded"]', 10000);
    expect(appLoadedIndicator).toBeTruthy();
    console.log('[ONBOARDING] ✓ App loaded directly (onboarding skipped)');
    
    // Verify no onboarding UI is present
    const hasOnboardingStep = await tauriPage.isVisible('text=Welcome') || 
                               await tauriPage.isVisible('text=Get Started') ||
                               await tauriPage.isVisible('.onboarding-wizard, [class*="onboarding"]');
    
    expect(hasOnboardingStep).toBeFalsy();
    console.log('[ONBOARDING] ✓ No onboarding UI detected');
  });
});
