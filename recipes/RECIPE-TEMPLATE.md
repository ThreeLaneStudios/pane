---
name: ""
type: ""            # "agent" or "job" — must match the subfolder this file lives in
description: ""      # one or two sentences: what it does
touches:
  files: []          # paths it reads or writes, or [] if none
  network: []        # domains/services it calls, or [] if none
  commands: []        # binaries/scripts it invokes, or [] if none
---

## What it does

<!-- Same as the "description" above, expanded. Plain language, no jargon. -->

## The prompt

```text
<!-- The exact prompt text. This is what actually runs. -->
```

## What it touches

<!-- PR route: prose restatement of the frontmatter touches: block, written by
     the contributor, matching the frontmatter exactly -- if it doesn't, that's
     itself a review flag (see recipes/REVIEW-CHECKLIST.md, item 9).
     Issue-form route: pasted verbatim from the submitter's own "what-it-touches"
     field in .github/ISSUE_TEMPLATE/recipe-submission.yml -- not composed from
     their touches-files/touches-network/touches-commands answers. Either way, a
     mismatch with the frontmatter is a review flag, not something to silently
     fix by rewriting one side to match the other. -->
