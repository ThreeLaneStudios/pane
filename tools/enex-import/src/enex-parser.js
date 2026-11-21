/**
 * Streaming ENEX parser using SAX.
 * 
 * Parses Evernote's XML export format (.enex) as a stream so we can handle
 * archives with 5,000+ notes without running out of memory.
 * 
 * Emits 'note' events as each note is fully parsed.
 */

import sax from 'sax';
import { createReadStream } from 'fs';
import { EventEmitter } from 'events';
import { createHash } from 'crypto';

export class EnexParser extends EventEmitter {
  constructor() {
    super();
  }

  /**
   * Parse an ENEX file as a stream. Emits:
   *   - 'note' (noteObj) — for each parsed note
   *   - 'done' (count) — when parsing is complete
   *   - 'error' (err) — on parse errors
   * 
   * @param {string} filePath - Path to the .enex file
   * @param {string} [notebookName] - Override notebook name (defaults to filename)
   * @returns {Promise<number>} - Number of notes parsed
   */
  parse(filePath, notebookName) {
    return new Promise((resolve, reject) => {
      const stream = createReadStream(filePath, { encoding: 'utf8', highWaterMark: 64 * 1024 });
      const parser = sax.createStream(/*strict=*/false, {
        trim: false,
        normalize: false,
        lowercase: true,
        xmlns: false,
        position: true,
      });

      let noteCount = 0;
      let currentNote = null;
      let currentResource = null;
      let currentTag = null;
      let textBuffer = '';
      let tagStack = [];

      parser.on('opentag', (node) => {
        const name = node.name.toLowerCase();
        tagStack.push(name);
        textBuffer = '';

        if (name === 'note') {
          currentNote = {
            title: '',
            content: '',
            created: null,
            updated: null,
            tags: [],
            sourceUrl: null,
            author: null,
            latitude: null,
            longitude: null,
            altitude: null,
            resources: [],
            attributes: {},
          };
        } else if (name === 'resource' && currentNote) {
          currentResource = {
            data: '',
            mime: '',
            filename: '',
            width: null,
            height: null,
            hash: null,
          };
        }
      });

      parser.on('text', (text) => {
        textBuffer += text;
      });

      parser.on('cdata', (cdata) => {
        textBuffer += cdata;
      });

      parser.on('closetag', (name) => {
        name = name.toLowerCase();
        tagStack.pop();
        const parent = tagStack[tagStack.length - 1];

        if (name === 'note' && currentNote) {
          // Compute resource hashes for any resources missing them
          for (const res of currentNote.resources) {
            if (!res.hash && res.data) {
              const buf = Buffer.from(res.data, 'base64');
              res.hash = createHash('md5').update(buf).digest('hex');
            }
          }
          noteCount++;
          this.emit('note', currentNote);
          currentNote = null;
        } else if (name === 'resource' && currentResource && currentNote) {
          currentNote.resources.push(currentResource);
          currentResource = null;
        } else if (currentNote && !currentResource) {
          // Note-level fields
          switch (name) {
            case 'title':
              currentNote.title = textBuffer.trim();
              break;
            case 'content':
              currentNote.content = textBuffer;
              break;
            case 'created':
              currentNote.created = parseEvernoteDate(textBuffer.trim());
              break;
            case 'updated':
              currentNote.updated = parseEvernoteDate(textBuffer.trim());
              break;
            case 'tag':
              currentNote.tags.push(textBuffer.trim());
              break;
            case 'source-url':
              if (parent === 'note-attributes') {
                currentNote.sourceUrl = textBuffer.trim();
              }
              break;
            case 'author':
              if (parent === 'note-attributes') {
                currentNote.author = textBuffer.trim();
              }
              break;
            case 'latitude':
              if (parent === 'note-attributes') {
                currentNote.latitude = parseFloat(textBuffer.trim()) || null;
              }
              break;
            case 'longitude':
              if (parent === 'note-attributes') {
                currentNote.longitude = parseFloat(textBuffer.trim()) || null;
              }
              break;
            case 'altitude':
              if (parent === 'note-attributes') {
                currentNote.altitude = parseFloat(textBuffer.trim()) || null;
              }
              break;
          }
        } else if (currentResource) {
          // Resource-level fields
          switch (name) {
            case 'data':
              currentResource.data = textBuffer.replace(/\s/g, '');
              break;
            case 'mime':
              currentResource.mime = textBuffer.trim();
              break;
            case 'file-name':
              currentResource.filename = textBuffer.trim();
              break;
            case 'width':
              currentResource.width = parseInt(textBuffer.trim(), 10) || null;
              break;
            case 'height':
              currentResource.height = parseInt(textBuffer.trim(), 10) || null;
              break;
          }
          // Check for recognition data hash
          if (name === 'data' && parent === 'recognition') {
            // skip — not the resource data
          }
        }

        textBuffer = '';
      });

      parser.on('error', (err) => {
        // SAX is lenient — try to continue
        this.emit('warning', `Parse warning: ${err.message}`);
        parser._parser.error = null;
        parser._parser.resume();
      });

      parser.on('end', () => {
        this.emit('done', noteCount);
        resolve(noteCount);
      });

      stream.on('error', (err) => {
        this.emit('error', err);
        reject(err);
      });

      stream.pipe(parser);
    });
  }
}

/**
 * Parse Evernote's date format: 20260210T123456Z
 * @param {string} dateStr 
 * @returns {Date|null}
 */
function parseEvernoteDate(dateStr) {
  if (!dateStr || dateStr.length < 8) return null;

  // Format: YYYYMMDDTHHmmssZ
  const match = dateStr.match(/^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})Z?$/);
  if (match) {
    const [, y, m, d, h, min, s] = match;
    return new Date(Date.UTC(
      parseInt(y), parseInt(m) - 1, parseInt(d),
      parseInt(h), parseInt(min), parseInt(s)
    ));
  }

  // Try ISO fallback
  const date = new Date(dateStr);
  return isNaN(date.getTime()) ? null : date;
}

export default EnexParser;
