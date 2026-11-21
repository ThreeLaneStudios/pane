#!/usr/bin/env bash
# =============================================================================
# extract-run-block.sh — pull a single step's `run: |` block out of a
# GitHub Actions workflow YAML file, programmatically, by locating the
# step's `- name:` marker and then the fixed-indent `run: |` block that
# follows it — NOT by hand-copying the script text into a test file. A hand
# copy drifts from the real workflow the moment someone edits one and
# forgets the other, and then the test stops testing the real thing. This
# always re-reads the live file, so it fails loudly (empty output /
# non-zero exit) the moment the step is renamed or restructured, rather
# than silently testing stale text.
#
# Deliberately a small, auditable marker-slice (fixed 6/8/10-space YAML
# indentation, verified against this repo's actual workflow files) instead
# of a full YAML parse — no PyYAML/third-party dependency needed for a
# two-step workflow file shape that hasn't changed in this repo's history.
# If indentation ever changes, this will produce empty output and the
# calling test will fail loudly rather than silently passing on stale text.
#
# Usage: extract-run-block.sh <workflow.yml> <step-name>
# Prints the run: block's raw text (dedented) to stdout.
# =============================================================================
set -euo pipefail

if [ $# -ne 2 ]; then
  echo "usage: extract-run-block.sh <workflow.yml> <step-name>" >&2
  exit 2
fi

workflow="$1"
step_name="$2"

awk -v step_marker="      - name: ${step_name}" '
  BEGIN { in_step = 0; in_run = 0; found = 0 }
  {
    line = $0

    if (!in_step) {
      if (line == step_marker) { in_step = 1 }
      next
    }

    if (in_step && !in_run) {
      # Step-level key at 8-space indent that is not `run: |` ends the
      # search for this step without ever finding a run block.
      if (line == "        run: |") { in_run = 1; found = 1; next }
      if (line ~ /^        [a-zA-Z_]+:/) { exit }
      next
    }

    if (in_run) {
      if (substr(line, 1, 10) == "          ") {
        print substr(line, 11)
      } else if (line == "") {
        print ""
      } else {
        # Indentation dropped back to step level (or below) — block ended.
        exit
      }
    }
  }
  END {
    if (!found) {
      print "extract-run-block.sh: step " step_marker " / its run: block not found" > "/dev/stderr"
      exit 1
    }
  }
' "$workflow"
