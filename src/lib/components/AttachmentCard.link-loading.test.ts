/**
 * AttachmentCard.link-loading.test.ts — #1898 isLoading regression tests
 *
 * Verifies that the isLoading derivation correctly excludes link attachments
 * from the uploading-skeleton heuristic (which was designed for file uploads).
 *
 *  LCL-1  link attachment with no filename/attachmentId/status → isLoading false
 *         (skeleton NOT rendered)
 *  LCL-2  file attachment with no filename/attachmentId/status → isLoading TRUE
 *         (regression: skeleton still renders for real uploading files)
 *  LCL-3  file attachment with status: 'uploading' → isLoading true (regression)
 *  LCL-4  file attachment with attachmentId set → isLoading false (regression)
 *  LCL-5  link attachment with status: 'uploading' (edge case) → isLoading false
 *         (link is always excluded from the loading path)
 *
 * spec-skip: pure UI fix; no core-behavioral files touched; fixes isLoading
 *            heuristic to exclude link attachments per LOCKED-DESIGN-SPEC.md
 *            (Reed audit 2026-07-04-bug1-link-plain-skeleton.md)
 * Relates to #1898
 */

import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/svelte/svelte5';
import AttachmentCard from './AttachmentCard.svelte';

// ─── Helpers ──────────────────────────────────────────────────────────────────

/** Construct a link attachment prop object as passed by AttachmentNodeView */
function mkLinkAttachment(overrides: Record<string, unknown> = {}) {
  return {
    attachmentId: null,
    attachmentType: 'link',
    filename: null,
    src: 'https://pane.so',
    alt: null,
    duration: null,
    pageCount: null,
    width: null,
    height: null,
    size: null,
    status: null,
    errorDetail: null,
    ...overrides,
  };
}

/** Construct a file attachment prop object as passed by AttachmentNodeView */
function mkFileAttachment(overrides: Record<string, unknown> = {}) {
  return {
    attachmentId: null,
    attachmentType: 'file',
    filename: null,
    src: null,
    alt: null,
    duration: null,
    pageCount: null,
    width: null,
    height: null,
    size: null,
    status: null,
    errorDetail: null,
    ...overrides,
  };
}

// ─── LCL-1: link attachment with null everything → NOT loading ────────────────

describe('LCL-1: link attachment with no filename/attachmentId/status → isLoading false', () => {
  it('does not render .is-loading skeleton', () => {
    const { container } = render(AttachmentCard, {
      props: {
        attachment: mkLinkAttachment({
          // all three conditions that triggered the old bug: null status, null filename, null attachmentId
          attachmentId: null,
          filename: null,
          status: null,
        }),
      },
    });

    expect(container.querySelector('.is-loading')).toBeNull();
    expect(container.querySelector('.at-skel')).toBeNull();
    expect(container.querySelector('.at-spin')).toBeNull();
  });

  it('renders .at-card (not skeleton) for link attachment with no metadata', () => {
    const { container } = render(AttachmentCard, {
      props: {
        attachment: mkLinkAttachment({
          attachmentId: null,
          filename: null,
          status: null,
        }),
      },
    });

    // The regular card block must be present
    expect(container.querySelector('.at-card')).not.toBeNull();
    // But it must NOT have is-loading
    const card = container.querySelector('.at-card');
    expect(card!.classList.contains('is-loading')).toBe(false);
  });
});

// ─── LCL-2: file attachment with null everything → isLoading TRUE (regression) ─

describe('LCL-2: file attachment with no filename/attachmentId/status → isLoading true (skeleton renders)', () => {
  it('renders .is-loading skeleton for file attachment with no metadata', () => {
    const { container } = render(AttachmentCard, {
      props: {
        attachment: mkFileAttachment({
          attachmentId: null,
          filename: null,
          status: null,
        }),
      },
    });

    // File upload heuristic: !status && !filename && !attachmentId → skeleton
    expect(container.querySelector('.is-loading')).not.toBeNull();
    expect(container.querySelector('.at-skel')).not.toBeNull();
    expect(container.querySelector('.at-spin')).not.toBeNull();
  });
});

// ─── LCL-3: file attachment with status 'uploading' → isLoading true ─────────

describe("LCL-3: file attachment with status 'uploading' → isLoading true", () => {
  it("renders skeleton when status is 'uploading'", () => {
    const { container } = render(AttachmentCard, {
      props: {
        attachment: mkFileAttachment({
          status: 'uploading',
          filename: 'my-upload.pdf',
          attachmentId: null,
        }),
      },
    });

    expect(container.querySelector('.is-loading')).not.toBeNull();
    expect(container.querySelector('.at-spin')).not.toBeNull();
  });
});

// ─── LCL-4: file attachment with attachmentId set → isLoading false ───────────

describe('LCL-4: file attachment with attachmentId set → isLoading false', () => {
  it('does not render skeleton when attachmentId is present', () => {
    const { container } = render(AttachmentCard, {
      props: {
        attachment: mkFileAttachment({
          attachmentId: 'att-uuid-abc123',
          filename: 'report.pdf',
          status: null,
          size: 204800,
        }),
      },
    });

    expect(container.querySelector('.is-loading')).toBeNull();
    expect(container.querySelector('.at-card')).not.toBeNull();
  });
});

// ─── LCL-5: link attachment with status 'uploading' (edge case) → isLoading false

describe("LCL-5: link attachment with status 'uploading' (edge case) → isLoading false", () => {
  it("link is excluded from loading path even if status is 'uploading'", () => {
    const { container } = render(AttachmentCard, {
      props: {
        attachment: mkLinkAttachment({
          // Edge case: this state shouldn't happen in practice, but if it did,
          // the link exclusion guard must still prevent the skeleton
          status: 'uploading',
          attachmentId: null,
          filename: null,
        }),
      },
    });

    // Link nodes are always excluded — no skeleton regardless of status
    expect(container.querySelector('.is-loading')).toBeNull();
    expect(container.querySelector('.at-skel')).toBeNull();
    expect(container.querySelector('.at-spin')).toBeNull();
  });
});
