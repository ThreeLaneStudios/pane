/**
 * Note writer — saves converted notes as Markdown files with YAML frontmatter.
 * 
 * Output structure:
 *   ~/Trove/notebooks/{notebook-name}/{note-slug}.md
 *   ~/Trove/.trove/attachments/{note-id}/{filename}
 */

import { mkdir, writeFile, utimes } from 'fs/promises';
import { join, extname } from 'path';
import slugify from 'slugify';
import { stringify as yamlStringify } from 'yaml';
import { createHash } from 'crypto';

/**
 * Generate a URL-safe slug from a note title.
 */
export function makeSlug(title) {
  if (!title || !title.trim()) return 'untitled';

  let slug = slugify(title, {
    lower: true,
    strict: true,
    trim: true,
  });

  // If slugify returns empty (e.g. all special chars), hash the title
  if (!slug) {
    slug = 'note-' + createHash('md5').update(title).digest('hex').slice(0, 8);
  }

  // Cap length
  if (slug.length > 80) {
    slug = slug.slice(0, 80).replace(/-+$/, '');
  }

  return slug;
}

/**
 * Generate a unique note ID based on title + created date.
 */
export function makeNoteId(title, created) {
  const src = `${title || 'untitled'}-${created?.toISOString() || Date.now()}`;
  return createHash('md5').update(src).digest('hex').slice(0, 12);
}

/**
 * Build YAML frontmatter for a note.
 */
export function buildFrontmatter(note) {
  const fm = {};

  fm.title = note.title || 'Untitled';

  if (note.tags && note.tags.length > 0) {
    fm.tags = note.tags;
  }

  if (note.created) {
    fm.created = note.created.toISOString();
  }

  if (note.updated) {
    fm.updated = note.updated.toISOString();
  }

  if (note.sourceUrl) {
    fm.source_url = note.sourceUrl;
  }

  if (note.author) {
    fm.author = note.author;
  }

  if (note.latitude && note.longitude) {
    fm.location = {
      latitude: note.latitude,
      longitude: note.longitude,
    };
    if (note.altitude) {
      fm.location.altitude = note.altitude;
    }
  }

  return '---\n' + yamlStringify(fm).trim() + '\n---\n';
}

/**
 * Determine the file extension for a given MIME type.
 */
function extForMime(mime) {
  const map = {
    'image/jpeg': '.jpg',
    'image/png': '.png',
    'image/gif': '.gif',
    'image/webp': '.webp',
    'image/svg+xml': '.svg',
    'image/tiff': '.tiff',
    'image/bmp': '.bmp',
    'application/pdf': '.pdf',
    'audio/mpeg': '.mp3',
    'audio/wav': '.wav',
    'audio/ogg': '.ogg',
    'audio/amr': '.amr',
    'video/mp4': '.mp4',
    'video/quicktime': '.mov',
    'text/plain': '.txt',
    'text/html': '.html',
    'text/csv': '.csv',
    'application/msword': '.doc',
    'application/vnd.openxmlformats-officedocument.wordprocessingml.document': '.docx',
    'application/vnd.ms-excel': '.xls',
    'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet': '.xlsx',
    'application/vnd.ms-powerpoint': '.ppt',
    'application/vnd.openxmlformats-officedocument.presentationml.presentation': '.pptx',
    'application/zip': '.zip',
    'application/x-tar': '.tar',
    'application/gzip': '.gz',
  };
  return map[mime] || '';
}

/**
 * Write a single note and its attachments to disk.
 * 
 * @param {object} options
 * @param {object} options.note - Parsed note object from EnexParser
 * @param {string} options.markdown - Converted Markdown content
 * @param {string} options.vaultPath - Path to the Trove vault root
 * @param {string} options.notebookName - Name of the notebook
 * @param {Map<string, object>} options.resourceMap - Resource map with savedFilename info
 * @returns {Promise<object>} Write result with details
 */
export async function writeNote({ note, markdown, vaultPath, notebookName, resourceMap }) {
  const noteId = makeNoteId(note.title, note.created);
  const slug = makeSlug(note.title);
  const notebookSlug = makeSlug(notebookName);
  const result = {
    noteId,
    slug,
    title: note.title,
    notebook: notebookName,
    filePath: '',
    attachmentsWritten: 0,
    warnings: [],
  };

  // Create notebook directory
  const notebookDir = join(vaultPath, 'notebooks', notebookSlug);
  await mkdir(notebookDir, { recursive: true });

  // Handle slug collisions by appending noteId
  let noteFilename = `${slug}.md`;
  let notePath = join(notebookDir, noteFilename);

  // Build the full file content
  const frontmatter = buildFrontmatter(note);
  const fileContent = frontmatter + '\n' + markdown;

  // Write the Markdown file
  await writeFile(notePath, fileContent, 'utf8');
  result.filePath = notePath;

  // Set file timestamps to match note dates
  if (note.created || note.updated) {
    const mtime = note.updated || note.created;
    const atime = note.created || note.updated;
    try {
      await utimes(notePath, atime, mtime);
    } catch (e) {
      result.warnings.push(`Could not set timestamps: ${e.message}`);
    }
  }

  // Write attachments
  if (note.resources && note.resources.length > 0) {
    const attachDir = join(vaultPath, '.trove', 'attachments', noteId);
    await mkdir(attachDir, { recursive: true });

    for (const resource of note.resources) {
      if (!resource.data) {
        result.warnings.push(`Empty resource data for ${resource.filename || resource.hash}`);
        continue;
      }

      try {
        // Determine filename
        let filename = resource.filename;
        if (!filename) {
          const ext = extForMime(resource.mime) || '';
          filename = (resource.hash || `attachment-${result.attachmentsWritten}`) + ext;
        }
        // Ensure unique by prepending hash if needed
        resource.savedFilename = filename;

        // Update the resourceMap so the converter can reference the right filename
        if (resource.hash && resourceMap) {
          const existing = resourceMap.get(resource.hash);
          if (existing) {
            existing.savedFilename = filename;
          }
        }

        const attachPath = join(attachDir, filename);
        const buffer = Buffer.from(resource.data, 'base64');
        await writeFile(attachPath, buffer);
        result.attachmentsWritten++;

        // Free the base64 data from memory
        resource.data = null;
      } catch (e) {
        result.warnings.push(`Failed to write attachment ${resource.filename}: ${e.message}`);
      }
    }
  }

  return result;
}

/**
 * Build a resource map from a note's resources (MD5 hash → resource info).
 */
export function buildResourceMap(resources) {
  const map = new Map();
  for (const res of resources) {
    if (res.hash) {
      map.set(res.hash, res);
    } else if (res.data) {
      // Compute hash if missing
      const buf = Buffer.from(res.data, 'base64');
      const hash = createHash('md5').update(buf).digest('hex');
      res.hash = hash;
      map.set(hash, res);
    }
  }
  return map;
}

export default { writeNote, makeSlug, makeNoteId, buildFrontmatter, buildResourceMap };
