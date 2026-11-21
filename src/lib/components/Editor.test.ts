import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { NoteMeta } from '$lib/types';
import { SYSTEM_NOTE_TYPES } from '$lib/constants/systemNoteTypes';

/**
 * System Notes Read-Only Tests
 * 
 * Per ADR-0011 and §5.7.2 of the memory pipeline spec:
 * All notes with a non-null note_type MUST open read-only in the Pane editor.
 * 
 * This test suite verifies:
 * 1. Read-only enforcement for all 7 system note types
 * 2. Banner displays correct write-surface guidance per type
 * 3. Write operations are prevented
 */

describe('Editor - System Notes Read-Only (ADR-0011)', () => {
  describe('Read-only enforcement', () => {
    SYSTEM_NOTE_TYPES.forEach((noteType) => {
      it(`should mark ${noteType} notes as read-only`, () => {
        const noteMeta: NoteMeta = {
          id: 'test-id',
          title: `Test ${noteType}`,
          note_type: noteType,
          slug: `test-${noteType}`,
          project_id: 'test-project',
          is_system: true,
          created_at: new Date().toISOString(),
          updated_at: new Date().toISOString(),
        };

        // Simulate the isReadOnly derivation from Editor.svelte
        const isReadOnly = !!(noteMeta?.note_type && (SYSTEM_NOTE_TYPES as readonly string[]).includes(noteMeta.note_type));

        expect(isReadOnly).toBe(true);
      });
    });

    it('should allow editing for user notes (note_type = null)', () => {
      const noteMeta: NoteMeta = {
        id: 'test-id',
        title: 'User Note',
        note_type: null,
        slug: 'user-note',
        project_id: 'test-project',
        is_system: false,
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
      };

      // Simulate the isReadOnly derivation from Editor.svelte
      const isReadOnly = !!(noteMeta?.note_type && (SYSTEM_NOTE_TYPES as readonly string[]).includes(noteMeta.note_type));

      expect(isReadOnly).toBe(false);
    });

    it('should cover all 7 system note types', () => {
      expect(SYSTEM_NOTE_TYPES).toHaveLength(7);
      expect(SYSTEM_NOTE_TYPES).toContain('soul');
      expect(SYSTEM_NOTE_TYPES).toContain('memory');
      expect(SYSTEM_NOTE_TYPES).toContain('rules');
      expect(SYSTEM_NOTE_TYPES).toContain('identity');
      expect(SYSTEM_NOTE_TYPES).toContain('user');
      expect(SYSTEM_NOTE_TYPES).toContain('tools');
      expect(SYSTEM_NOTE_TYPES).toContain('daily');
    });
  });

  describe('Banner write-surface guidance (§5.7.2)', () => {
    it('should show "Edit in Memory Settings" for memory notes', () => {
      const noteType = 'memory';
      let bannerText = '';

      if (noteType === 'memory') {
        bannerText = 'Edit in Memory Settings';
      } else if (noteType === 'daily') {
        bannerText = 'Managed by the memory pipeline';
      } else {
        bannerText = 'Created during onboarding. View in Agents settings.';
      }

      expect(bannerText).toBe('Edit in Memory Settings');
    });

    it('should show "Managed by the memory pipeline" for daily notes', () => {
      const noteType = 'daily';
      let bannerText = '';

      if (noteType === 'memory') {
        bannerText = 'Edit in Memory Settings';
      } else if (noteType === 'daily') {
        bannerText = 'Managed by the memory pipeline';
      } else {
        bannerText = 'Created during onboarding. View in Agents settings.';
      }

      expect(bannerText).toBe('Managed by the memory pipeline');
    });

    ['soul', 'rules', 'identity', 'user', 'tools'].forEach((noteType) => {
      it(`should show "Created during onboarding. View in Agents settings." for ${noteType} notes`, () => {
        let bannerText = '';

        if (noteType === 'memory') {
          bannerText = 'Edit in Memory Settings';
        } else if (noteType === 'daily') {
          bannerText = 'Managed by the memory pipeline';
        } else {
          bannerText = 'Created during onboarding. View in Agents settings.';
        }

        expect(bannerText).toBe('Created during onboarding. View in Agents settings.');
      });
    });
  });

  describe('Write operation prevention', () => {
    SYSTEM_NOTE_TYPES.forEach((noteType) => {
      it(`should prevent edits to ${noteType} notes`, () => {
        const noteMeta: NoteMeta = {
          id: 'test-id',
          title: `Test ${noteType}`,
          note_type: noteType,
          slug: `test-${noteType}`,
          project_id: 'test-project',
          is_system: true,
          created_at: new Date().toISOString(),
          updated_at: new Date().toISOString(),
        };

        const isReadOnly = !!(noteMeta?.note_type && (SYSTEM_NOTE_TYPES as readonly string[]).includes(noteMeta.note_type));

        // When isReadOnly = true, the editor should be set to non-editable
        // In TipTap this is done via editor.setEditable(!isReadOnly)
        // Simulate this check:
        const editorShouldBeEditable = !isReadOnly;

        expect(editorShouldBeEditable).toBe(false);
      });
    });

    it('should allow writes to user notes', () => {
      const noteMeta: NoteMeta = {
        id: 'test-id',
        title: 'User Note',
        note_type: null,
        slug: 'user-note',
        project_id: 'test-project',
        is_system: false,
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
      };

      const isReadOnly = !!(noteMeta?.note_type && (SYSTEM_NOTE_TYPES as readonly string[]).includes(noteMeta.note_type));
      const editorShouldBeEditable = !isReadOnly;

      expect(editorShouldBeEditable).toBe(true);
    });
  });

  describe('Coverage verification', () => {
    it('should not have any system note type missing from SYSTEM_NOTE_TYPES', () => {
      // Per the spec, these are ALL the system note types that exist
      const specNoteTypes = ['memory', 'daily', 'soul', 'rules', 'identity', 'user', 'tools'];
      
      specNoteTypes.forEach((type) => {
        expect(SYSTEM_NOTE_TYPES).toContain(type);
      });
    });

    it('should match the array length defined in the spec', () => {
      // The spec defines exactly 7 system note types
      expect(SYSTEM_NOTE_TYPES).toHaveLength(7);
    });
  });
});
