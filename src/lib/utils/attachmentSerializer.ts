/**
 * Pure attachment serialization helpers.
 *
 * Extracted from Editor.svelte (`serializeAttachmentWrapper`) to enable unit testing
 * of the raw-HTML block builder without instantiating the Svelte component.
 *
 * The serializer behavior is UNCHANGED — this is a structural extraction only.
 * Editor.svelte's `serializeAttachmentWrapper` delegates to `buildAttachmentRawHtmlBlock`
 * for the non-default-mode (raw HTML block) path.
 *
 * Relates to #401 PR5b — fixes attachment metadata drop on remount.
 */

/** HTML-escape a string for safe emission inside attribute values. */
export function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#039;');
}

/** Extract the file extension (including dot) from a filename. e.g. 'photo.jpg' → '.jpg'. */
export function extOf(filename: string): string {
  const dot = filename.lastIndexOf('.');
  return dot >= 0 ? filename.substring(dot) : '';
}

/**
 * Build the raw-HTML block for a non-default-mode attachment wrapper.
 *
 * Belt-and-suspenders per the burn-area comment in Editor.svelte:serializeAttachmentWrapper:
 * always emit data-filename + data-size when present so metadata survives the round-trip
 * through the flat .md file even if a non-default render mode is stored.
 *
 * (#401 PR5b — previously these two attrs were omitted, causing AttachmentCard to show
 *  generic "File / AUDIO" on remount for every drag/paste audio upload.)
 */
export function buildAttachmentRawHtmlBlock(params: {
  attachmentId: string;
  renderMode: string | null;
  attachmentType: string | null;
  /** data-filename attr from the wrapper element — belt-and-suspenders for metadata. */
  filenameAttr: string | null;
  /** data-size attr from the wrapper element — belt-and-suspenders for metadata. */
  sizeAttr: string | null;
  widthAttr: string | null;
  heightAttr: string | null;
  /** Inner HTML of the wrapper element (NodeView children, often empty). */
  inner: string;
}): string {
  const { attachmentId, renderMode, attachmentType, filenameAttr, sizeAttr, widthAttr, heightAttr, inner } = params;
  let wrapperOpen = `<div data-attachment data-attachment-id="${escapeHtml(attachmentId)}"`;
  if (renderMode)     wrapperOpen += ` data-render-mode="${escapeHtml(renderMode)}"`;
  if (attachmentType) wrapperOpen += ` data-attachment-type="${escapeHtml(attachmentType)}"`;
  if (filenameAttr)   wrapperOpen += ` data-filename="${escapeHtml(filenameAttr)}"`;
  if (sizeAttr)       wrapperOpen += ` data-size="${escapeHtml(sizeAttr)}"`;
  if (widthAttr)      wrapperOpen += ` data-width="${escapeHtml(widthAttr)}"`;
  if (heightAttr)     wrapperOpen += ` data-height="${escapeHtml(heightAttr)}"`;
  wrapperOpen += `>`;
  return `${wrapperOpen}${inner}</div>\n\n`;
}
