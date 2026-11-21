# Trove ENEX Import Tool

Import Evernote's `.enex` export files into Trove's Markdown vault format.

## Features

- **Streaming XML parser** — handles 5,000+ note archives without running out of memory
- **Full ENML → Markdown conversion** — headings, bold, italic, lists, checklists, tables, code blocks, links, blockquotes, horizontal rules
- **Attachment extraction** — images, PDFs, and other files saved alongside notes
- **Metadata preservation** — tags, created/updated dates, source URLs, author, geolocation
- **YAML frontmatter** — each note includes structured metadata
- **File timestamps** — created/modified dates set from Evernote metadata
- **Progress reporting** — real-time progress bar for large imports
- **Import report** — summary of successes, warnings, and errors

## Installation

```bash
cd ~/Projects/trove/tools/enex-import
npm install
```

## Usage

### Basic Import

```bash
node import.js --input path/to/export.enex --vault ~/Trove
```

### Options

| Option | Short | Description |
|---|---|---|
| `--input <path>` | `-i` | Path to the `.enex` file (required) |
| `--vault <path>` | `-v` | Path to Trove vault root (required) |
| `--notebook <name>` | `-n` | Override notebook name (defaults to filename) |
| `--dry-run` | | Parse and convert without writing files |
| `--verbose` | | Show per-note output during import |
| `--quiet` | `-q` | Suppress progress bar |

### Examples

```bash
# Import with custom notebook name
node import.js -i evernote-backup.enex -v ~/Trove -n "Personal Notes"

# Dry run to preview what will be imported
node import.js -i huge-archive.enex -v ~/Trove --dry-run --verbose

# Quiet mode for scripts
node import.js -i export.enex -v ~/Trove -q
```

## Output Structure

```
~/Trove/
├── notebooks/
│   └── {notebook-name}/
│       ├── welcome-to-trove.md
│       ├── meeting-notes-2026-02-10.md
│       └── ...
└── .trove/
    └── attachments/
        └── {note-id}/
            ├── photo.jpg
            ├── document.pdf
            └── ...
```

### Note Format

Each note is saved as a Markdown file with YAML frontmatter:

```markdown
---
title: My Note Title
tags:
  - tag1
  - tag2
created: 2026-01-01T09:00:00.000Z
updated: 2026-02-10T12:00:00.000Z
source_url: https://example.com
author: Matt
---

# My Note Title

The note content in clean Markdown...
```

## How It Works

1. **Parse** — Streams the ENEX XML file using SAX, extracting notes one at a time
2. **Convert** — Transforms Evernote's ENML (a subset of XHTML) to Markdown via Turndown
3. **Extract** — Decodes base64 attachments and saves them to the vault
4. **Write** — Creates Markdown files with frontmatter and sets file timestamps

### ENML Element Handling

| ENML Element | Markdown Output |
|---|---|
| `<en-note>` | Document root (unwrapped) |
| `<en-todo checked="true">` | `- [x]` checkbox |
| `<en-todo checked="false">` | `- [ ]` checkbox |
| `<en-media type="image/*">` | `![alt](path)` image |
| `<en-media type="*">` | `[filename](path)` link |
| `<en-crypt>` | ⚠️ encrypted content warning |

## Testing

```bash
# Run all tests
node --test test/test-import.js

# Run against sample ENEX
node import.js -i test/sample.enex -v /tmp/trove-test --verbose
```

The test suite includes:
- Unit tests for slugification, ID generation, frontmatter building
- ENEX parser tests (titles, tags, dates, attributes, resources)
- ENML converter tests (all formatting types)
- Full integration test (end-to-end import and file verification)

## Tech Stack

- **sax** — Streaming XML parser for memory-efficient ENEX processing
- **turndown** + **turndown-plugin-gfm** — HTML/ENML to Markdown conversion
- **commander** — CLI argument parsing
- **cli-progress** — Progress bar for large imports
- **yaml** — YAML frontmatter generation
- **slugify** — URL-safe filename generation

## Limitations

- **Encrypted content** (`<en-crypt>`) cannot be decrypted without the user's passphrase
- **Ink notes** (handwriting) are not supported
- **Internal links** between notes are preserved as-is but not remapped to Trove paths
- Heavily styled notes (lots of inline CSS) may have minor formatting differences
