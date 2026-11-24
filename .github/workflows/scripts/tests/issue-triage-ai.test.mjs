// Unit tests for issue-triage-ai.mjs's pure/testable surface. No network:
// GitHub and Anthropic calls are injected fakes (same "testing seam" shape
// as scripts/check-spec-first.sh's fixtures/gh). Run:
// node --test .github/workflows/scripts/tests/
import test from "node:test";
import assert from "node:assert/strict";
import {
  VALID_LABELS,
  MARKER_LABEL,
  FOOTER,
  filterLabels,
  parseClassifyResponse,
  parseSpecCheckResponse,
  selectFixedTemplate,
  composeReply,
  isTeamPermission,
  isAlreadyRepliedTo,
  makeWriteGate,
  wrapUntrusted,
  processIssue,
  ensureLabel,
  shouldSkipTeamCheckForTargetedIssues,
  validateIssueNumbersMode,
} from "../issue-triage-ai.mjs";
import { buildSections } from "../extract-spec-headings.mjs";

// ─── No em dashes anywhere in prompt/reply-facing constants (amendment 2) ──

test("FOOTER contains no em dash", () => {
  assert.equal(FOOTER.includes("\u2014"), false);
});

// ─── filterLabels / parseClassifyResponse (8a: off-menu label) ────────────

test("filterLabels keeps only the exact seven, drops and reports the rest", () => {
  const { valid, dropped } = filterLabels(["bug", "urgent", "feature", 42, "feature"]);
  assert.deepEqual(valid, ["bug", "feature"]);
  assert.deepEqual(dropped, ["urgent", 42]);
});

test("filterLabels tolerates a non-array input", () => {
  assert.deepEqual(filterLabels(undefined), { valid: [], dropped: [] });
  assert.deepEqual(filterLabels(null), { valid: [], dropped: [] });
});

test("VALID_LABELS is exactly the six remaining after #445 removed needs-spec-section, in order", () => {
  assert.deepEqual(VALID_LABELS, ["bug", "feature", "question", "needs-repro", "spam", "duplicate"]);
});

test("parseClassifyResponse rejects malformed JSON", () => {
  const r = parseClassifyResponse("not json at all");
  assert.equal(r.ok, false);
  assert.match(r.reason, /malformed JSON/);
});

test("parseClassifyResponse rejects a JSON array or primitive (not an object)", () => {
  assert.equal(parseClassifyResponse("[1,2,3]").ok, false);
  assert.equal(parseClassifyResponse('"a string"').ok, false);
});

test("parseClassifyResponse strips a markdown code fence defensively", () => {
  const r = parseClassifyResponse('```json\n{"labels":["bug"],"spec_section":null}\n```');
  assert.equal(r.ok, true);
  assert.deepEqual(r.labels, ["bug"]);
});

test("parseClassifyResponse passes through spec_section null or string, drops anything else to null", () => {
  assert.equal(parseClassifyResponse('{"labels":[],"spec_section":null}').specSectionRaw, null);
  assert.equal(parseClassifyResponse('{"labels":[],"spec_section":"## X"}').specSectionRaw, "## X");
  assert.equal(parseClassifyResponse('{"labels":[],"spec_section":42}').specSectionRaw, null);
});

// ─── parseSpecCheckResponse (8a: internal consistency) ─────────────────────

test("parseSpecCheckResponse rejects malformed JSON", () => {
  assert.equal(parseSpecCheckResponse("nope").ok, false);
});

test("parseSpecCheckResponse rejects addressed=true with a null reply (inconsistent)", () => {
  const r = parseSpecCheckResponse('{"addressed":true,"verdict":"matches","reply":null}');
  assert.equal(r.ok, false);
  assert.match(r.reason, /inconsistent/);
});

test("parseSpecCheckResponse rejects addressed=false with a non-null reply (inconsistent)", () => {
  const r = parseSpecCheckResponse('{"addressed":false,"verdict":"not_addressed","reply":"hello"}');
  assert.equal(r.ok, false);
  assert.match(r.reason, /inconsistent/);
});

test("parseSpecCheckResponse accepts a consistent addressed=true", () => {
  const r = parseSpecCheckResponse('{"addressed":true,"verdict":"matches","reply":"Spec section: ..."}');
  assert.equal(r.ok, true);
  assert.equal(r.addressed, true);
  assert.equal(r.reply, "Spec section: ...");
});

test("parseSpecCheckResponse accepts a consistent addressed=false", () => {
  const r = parseSpecCheckResponse('{"addressed":false,"verdict":"not_addressed","reply":null}');
  assert.equal(r.ok, true);
  assert.equal(r.reply, null);
});

// ─── Amendment 1: fixed-template selection ─────────────────────────────────

test("selectFixedTemplate picks needs-repro when it applies alongside an unrelated label", () => {
  const t = selectFixedTemplate(["bug", "needs-repro"]);
  assert.equal(t.kind, "needs-repro");
});

test("selectFixedTemplate returns null when needs-repro does not apply (e.g. bug+spam only, or #445's removed needs-spec-section)", () => {
  assert.equal(selectFixedTemplate(["bug", "spam"]), null);
  assert.equal(selectFixedTemplate([]), null);
  // needs-spec-section is no longer a VALID_LABELS member (#445); filterLabels
  // would already have dropped it before this function ever saw it, but confirm
  // selectFixedTemplate itself no longer recognizes it either.
  assert.equal(selectFixedTemplate(["needs-spec-section"]), null);
});

test("fixed template body contains no em dash (amendment 2)", () => {
  const repro = selectFixedTemplate(["needs-repro"]);
  assert.equal(repro.body.includes("\u2014"), false);
  assert.match(repro.body, /version.*platform.*steps.*expected/is);
});

// ─── composeReply: footer always appended exactly once, no em dash ────────

test("composeReply appends the fixed footer", () => {
  const r = composeReply("Some reply text.");
  assert.ok(r.includes("Some reply text."));
  assert.ok(r.endsWith(FOOTER));
});

test("composeReply does not double the footer if the model already included it", () => {
  const r = composeReply(`Some reply text.\n\n${FOOTER}`);
  const occurrences = r.split(FOOTER).length - 1;
  assert.equal(occurrences, 1);
});

// ─── isTeamPermission: repo-collaborator-permission classifier, not org ───

test("isTeamPermission treats write/maintain/admin as team", () => {
  assert.equal(isTeamPermission("write"), true);
  assert.equal(isTeamPermission("maintain"), true);
  assert.equal(isTeamPermission("admin"), true);
});

test("isTeamPermission treats triage/read/none/unknown as outside", () => {
  assert.equal(isTeamPermission("triage"), false);
  assert.equal(isTeamPermission("read"), false);
  assert.equal(isTeamPermission("none"), false);
  assert.equal(isTeamPermission(undefined), false);
});

// ─── isAlreadyRepliedTo: dedupe checked against the thread, not just the label ──

test("isAlreadyRepliedTo finds the marker inside any existing comment body", () => {
  const comments = [{ body: "unrelated" }, { body: `Spec section: foo.\n\n${FOOTER}` }];
  assert.equal(isAlreadyRepliedTo(comments), true);
});

test("isAlreadyRepliedTo is false when no comment carries the footer", () => {
  assert.equal(isAlreadyRepliedTo([{ body: "unrelated" }]), false);
  assert.equal(isAlreadyRepliedTo([]), false);
});

// ─── wrapUntrusted: Pane's own delimiter convention, reused verbatim ──────

test("wrapUntrusted matches src-tauri/src/ai/web.rs's EXTERNAL_UNTRUSTED_CONTENT shape", () => {
  const wrapped = wrapUntrusted("issue:owner/repo#1", "body text");
  assert.equal(wrapped, '<<<EXTERNAL_UNTRUSTED_CONTENT source="issue:owner/repo#1">>>\nbody text\n<<<END_EXTERNAL_UNTRUSTED_CONTENT>>>');
});

// ─── makeWriteGate: dry-run never calls fn, never increments the counter ──

test("makeWriteGate in dry-run mode never invokes fn and never increments writesPerformed", async () => {
  const gate = makeWriteGate(true);
  let called = false;
  const result = await gate.maybeWrite("would do X", () => {
    called = true;
    return "real result";
  });
  assert.equal(called, false);
  assert.equal(result, null);
  assert.equal(gate.writesPerformed, 0);
  assert.equal(gate.summaryEvents.length, 1);
  assert.equal(gate.summaryEvents[0].dryRun, true);
});

test("makeWriteGate in live mode invokes fn and increments writesPerformed", async () => {
  const gate = makeWriteGate(false);
  const result = await gate.maybeWrite("do X", () => "real result");
  assert.equal(result, "real result");
  assert.equal(gate.writesPerformed, 1);
  assert.equal(gate.summaryEvents[0].dryRun, false);
});

// ─── ensureLabel: check-then-create, cached per run ────────────────────────

test("ensureLabel does not create a label that already exists", async () => {
  const calls = [];
  const gh = {
    getLabel: async (name) => {
      calls.push(`get:${name}`);
      return { name };
    },
    createLabel: async (name) => {
      calls.push(`create:${name}`);
    },
  };
  const gate = makeWriteGate(false);
  await ensureLabel("bug", gh, gate, new Map());
  assert.deepEqual(calls, ["get:bug"]);
});

test("ensureLabel creates a missing label exactly once, then uses the per-run cache", async () => {
  const calls = [];
  const gh = {
    getLabel: async (name) => {
      calls.push(`get:${name}`);
      return null;
    },
    createLabel: async (name) => {
      calls.push(`create:${name}`);
    },
  };
  const gate = makeWriteGate(false);
  const cache = new Map();
  await ensureLabel("feature", gh, gate, cache);
  await ensureLabel("feature", gh, gate, cache);
  assert.deepEqual(calls, ["get:feature", "create:feature"]); // second call hit the cache, no second get/create
});

test("ensureLabel in dry-run mode logs a would-create line and never calls createLabel", async () => {
  const calls = [];
  const gh = {
    getLabel: async () => null,
    createLabel: async (name) => calls.push(`create:${name}`),
  };
  const gate = makeWriteGate(true);
  await ensureLabel("spam", gh, gate, new Map());
  assert.deepEqual(calls, []);
  assert.equal(gate.summaryEvents.some((e) => e.description.includes("createLabel(spam)")), true);
});

// ─── processIssue: end-to-end with fake gh/anthropic, dry-run, proves ordering ──

function fakeGh({ permission, comments = [], labelsExist = {} }) {
  const addLabelsCalls = [];
  const createCommentCalls = [];
  const createLabelCalls = [];
  return {
    addLabelsCalls,
    createCommentCalls,
    createLabelCalls,
    getPermission: async () => ({ permission }),
    listComments: async () => comments,
    getLabel: async (name) => (labelsExist[name] ? { name } : null),
    createLabel: async (name) => {
      createLabelCalls.push(name);
    },
    addLabels: async (issueNumber, labels) => {
      addLabelsCalls.push({ issueNumber, labels });
    },
    createComment: async (issueNumber, body) => {
      createCommentCalls.push({ issueNumber, body });
    },
  };
}

function fakeAnthropic(responses) {
  let i = 0;
  const calls = [];
  return {
    calls,
    call: async (system, user) => {
      calls.push({ system, user });
      const r = responses[i];
      i += 1;
      return r;
    },
  };
}

test("processIssue skips a team author without calling the model at all", async () => {
  const gh = fakeGh({ permission: "write" });
  const anthropic = fakeAnthropic([]);
  const gate = makeWriteGate(true);
  const issue = { number: 1, user: { login: "britfreed" }, title: "t", body: "b", labels: [] };
  const result = await processIssue(issue, {
    gh,
    anthropic,
    spec: { headingListText: "", sections: new Map() },
    owner: "o",
    repo: "r",
    writeGate: gate,
    labelCache: new Map(),
    log: () => {},
  });
  assert.equal(result.outcome, "skipped-team-author");
  assert.equal(anthropic.calls.length, 0);
  assert.equal(gh.addLabelsCalls.length, 0);
});

test("processIssue fails closed (skips) on a permission-check error", async () => {
  const gh = fakeGh({});
  gh.getPermission = async () => {
    throw new Error("network blip");
  };
  const anthropic = fakeAnthropic([]);
  const gate = makeWriteGate(true);
  const issue = { number: 2, user: { login: "someone" }, title: "t", body: "b", labels: [] };
  const result = await processIssue(issue, {
    gh,
    anthropic,
    spec: { headingListText: "", sections: new Map() },
    owner: "o",
    repo: "r",
    writeGate: gate,
    labelCache: new Map(),
    log: () => {},
  });
  assert.equal(result.outcome, "skipped-team-check-error");
  assert.equal(anthropic.calls.length, 0);
});

test("processIssue skips (and repairs the label) when the marker is already in the comment thread", async () => {
  const gh = fakeGh({ permission: "none", comments: [{ body: `already replied.\n\n${FOOTER}` }] });
  const anthropic = fakeAnthropic([]);
  const gate = makeWriteGate(false);
  const issue = { number: 3, user: { login: "outsider" }, title: "t", body: "b", labels: [] };
  const result = await processIssue(issue, {
    gh,
    anthropic,
    spec: { headingListText: "", sections: new Map() },
    owner: "o",
    repo: "r",
    writeGate: gate,
    labelCache: new Map(),
    log: () => {},
  });
  assert.equal(result.outcome, "already-processed");
  assert.equal(anthropic.calls.length, 0);
  assert.deepEqual(gh.addLabelsCalls, [{ issueNumber: 3, labels: [MARKER_LABEL] }]);
  assert.equal(gh.createCommentCalls.length, 0);
});

test("processIssue: Call 1 malformed JSON aborts the whole issue, no mutations at all", async () => {
  const gh = fakeGh({ permission: "none" });
  const anthropic = fakeAnthropic(["not json"]);
  const gate = makeWriteGate(false);
  const issue = { number: 4, user: { login: "outsider" }, title: "t", body: "b", labels: [] };
  const result = await processIssue(issue, {
    gh,
    anthropic,
    spec: { headingListText: "x.md: ## X", sections: new Map() },
    owner: "o",
    repo: "r",
    writeGate: gate,
    labelCache: new Map(),
    log: () => {},
  });
  assert.equal(result.outcome, "validation-failure");
  assert.equal(gh.addLabelsCalls.length, 0);
  assert.equal(gh.createCommentCalls.length, 0);
});

test("processIssue: amendment 1 fixed template fires when spec_section is null and needs-repro applies, labels then reply then marker, in order", async () => {
  const gh = fakeGh({ permission: "none" });
  const anthropic = fakeAnthropic([JSON.stringify({ labels: ["bug", "needs-repro"], spec_section: null })]);
  const gate = makeWriteGate(false);
  const issue = { number: 5, user: { login: "outsider" }, title: "t", body: "b", labels: [] };
  const result = await processIssue(issue, {
    gh,
    anthropic,
    spec: { headingListText: "x.md: ## X", sections: new Map() },
    owner: "o",
    repo: "r",
    writeGate: gate,
    labelCache: new Map(),
    log: () => {},
  });
  assert.equal(result.outcome, "processed");
  assert.equal(result.replyKind, "fixed-template:needs-repro");
  assert.ok(result.reply.includes("version"));
  assert.ok(result.reply.endsWith(FOOTER));
  // Order: classification labels, then the reply comment, then the marker label last.
  assert.equal(gh.addLabelsCalls.length, 2);
  assert.deepEqual(gh.addLabelsCalls[0].labels, ["bug", "needs-repro"]);
  assert.deepEqual(gh.addLabelsCalls[1].labels, [MARKER_LABEL]);
  assert.equal(gh.createCommentCalls.length, 1);
});

test("processIssue: Call 2 addressed=true posts the model's reply with the footer appended", async () => {
  const sections = buildSections([
    { file: "x.md", level: 2, text: "## X", startLine: 1, lines: ["# T", "## X", "body"] },
  ]);
  const gh = fakeGh({ permission: "none" });
  const anthropic = fakeAnthropic([
    JSON.stringify({ labels: ["bug"], spec_section: "## X" }),
    JSON.stringify({ addressed: true, verdict: "matches", reply: "Spec section: ## X, in `x.md`. It matches." }),
  ]);
  const gate = makeWriteGate(false);
  const issue = { number: 6, user: { login: "outsider" }, title: "t", body: "b", labels: [] };
  const result = await processIssue(issue, {
    gh,
    anthropic,
    spec: { headingListText: "x.md: ## X", sections },
    owner: "o",
    repo: "r",
    writeGate: gate,
    labelCache: new Map(),
    log: () => {},
  });
  assert.equal(result.outcome, "processed");
  assert.equal(result.replyKind, "call-2");
  assert.equal(anthropic.calls.length, 2);
  assert.ok(gh.createCommentCalls[0].body.includes("It matches."));
  assert.ok(gh.createCommentCalls[0].body.endsWith(FOOTER));
});

test("processIssue: Call 2 addressed=false with needs-repro falls back to the fixed template (amendment 1, the broader gap)", async () => {
  const sections = buildSections([
    { file: "x.md", level: 2, text: "## X", startLine: 1, lines: ["# T", "## X", "body"] },
  ]);
  const gh = fakeGh({ permission: "none" });
  const anthropic = fakeAnthropic([
    JSON.stringify({ labels: ["needs-repro"], spec_section: "## X" }),
    JSON.stringify({ addressed: false, verdict: "not_addressed", reply: null }),
  ]);
  const gate = makeWriteGate(false);
  const issue = { number: 7, user: { login: "outsider" }, title: "t", body: "b", labels: [] };
  const result = await processIssue(issue, {
    gh,
    anthropic,
    spec: { headingListText: "x.md: ## X", sections },
    owner: "o",
    repo: "r",
    writeGate: gate,
    labelCache: new Map(),
    log: () => {},
  });
  assert.equal(result.replyKind, "fixed-template:needs-repro");
  assert.ok(result.reply.includes("version"));
});

test("processIssue: Call 2 malformed JSON aborts the whole issue, including Call 1's labels", async () => {
  const sections = buildSections([
    { file: "x.md", level: 2, text: "## X", startLine: 1, lines: ["# T", "## X", "body"] },
  ]);
  const gh = fakeGh({ permission: "none" });
  const anthropic = fakeAnthropic([JSON.stringify({ labels: ["bug"], spec_section: "## X" }), "not json"]);
  const gate = makeWriteGate(false);
  const issue = { number: 8, user: { login: "outsider" }, title: "t", body: "b", labels: [] };
  const result = await processIssue(issue, {
    gh,
    anthropic,
    spec: { headingListText: "x.md: ## X", sections },
    owner: "o",
    repo: "r",
    writeGate: gate,
    labelCache: new Map(),
    log: () => {},
  });
  assert.equal(result.outcome, "validation-failure");
  assert.equal(gh.addLabelsCalls.length, 0);
  assert.equal(gh.createCommentCalls.length, 0);
});

test("processIssue: an invalid spec_section (not an exact heading match) is treated as null", async () => {
  const sections = buildSections([
    { file: "x.md", level: 2, text: "## X", startLine: 1, lines: ["# T", "## X", "body"] },
  ]);
  const gh = fakeGh({ permission: "none" });
  const anthropic = fakeAnthropic([JSON.stringify({ labels: ["feature"], spec_section: "## Does Not Exist" })]);
  const gate = makeWriteGate(false);
  const issue = { number: 9, user: { login: "outsider" }, title: "t", body: "b", labels: [] };
  const result = await processIssue(issue, {
    gh,
    anthropic,
    spec: { headingListText: "x.md: ## X", sections },
    owner: "o",
    repo: "r",
    writeGate: gate,
    labelCache: new Map(),
    log: () => {},
  });
  assert.equal(result.outcome, "processed");
  assert.equal(result.specSection, null);
  assert.equal(result.specSectionDropped, "## Does Not Exist");
  assert.equal(anthropic.calls.length, 1); // Call 2 never ran
  assert.equal(result.reply, null); // no needs-repro label, so no fixed template either
});

// ─── shouldSkipTeamCheckForTargetedIssues: the dry-run issue_numbers test hook (#3229) ───

test("the dry-run issue_numbers test hook is active ONLY for dry_run=true AND issue_numbers set AND the event is workflow_dispatch, and nowhere else", () => {
  // ON: the one case the hook exists for.
  assert.equal(
    shouldSkipTeamCheckForTargetedIssues({ isWorkflowDispatch: true, dryRun: true, issueNumbersRaw: "123,456" }),
    true,
  );

  // OFF: dry_run=true but issue_numbers not set (the normal manual dry run).
  assert.equal(shouldSkipTeamCheckForTargetedIssues({ isWorkflowDispatch: true, dryRun: true, issueNumbersRaw: "" }), false);
  assert.equal(shouldSkipTeamCheckForTargetedIssues({ isWorkflowDispatch: true, dryRun: true, issueNumbersRaw: "   " }), false);

  // OFF: dry_run=false with issue_numbers set. The hook's own guard is
  // independently false here (asserted below); separately, the existing
  // (pre-#3229) code already refuses this exact combination outright,
  // asserted via validateIssueNumbersMode so that refusal is also covered
  // by this test, not just narrated.
  assert.equal(shouldSkipTeamCheckForTargetedIssues({ isWorkflowDispatch: true, dryRun: false, issueNumbersRaw: "123" }), false);
  assert.throws(
    () => validateIssueNumbersMode({ issueNumbersRaw: "123", dryRun: false }),
    /only permitted in dry-run mode/,
  );

  // OFF: a scheduled-event shape. A real schedule trigger has no
  // workflow_dispatch inputs (ISSUE_NUMBERS would read as ""), but this
  // case is asserted with issueNumbersRaw deliberately NON-empty too, to
  // prove the guard explicitly checks isWorkflowDispatch and does not merely
  // rely on issue_numbers being structurally absent on that trigger. This
  // is the assertion that fails if someone later loosens the condition to
  // just "dryRun && issueNumbersRaw", dropping the trigger-type check.
  assert.equal(shouldSkipTeamCheckForTargetedIssues({ isWorkflowDispatch: false, dryRun: false, issueNumbersRaw: "" }), false);

  // OFF: a third, unknown GITHUB_EVENT_NAME shape (for example `issues` or
  // `issue_comment`, wired up at some point after this guard was written).
  // GITHUB_EVENT_NAME would derive isWorkflowDispatch=false for any event
  // that is not literally "workflow_dispatch", so this input is what such
  // a trigger looks like to this function, with dry_run=true and
  // issue_numbers deliberately non-empty, i.e. it would otherwise be the
  // ON case above if the trigger check were missing or wrong. This is
  // precisely the hole the allowlist closes: a denylist written against
  // today's two known triggers (isScheduleEvent !== true) would wrongly
  // return true for this unknown third trigger; the allowlist, keyed on
  // the single trigger the hook is actually built for, correctly returns
  // false regardless of what any future trigger happens to be named.
  assert.equal(shouldSkipTeamCheckForTargetedIssues({ isWorkflowDispatch: false, dryRun: true, issueNumbersRaw: "123" }), false);
});

// ─── processIssue: skipTeamCheck wiring (the hook's effect on a targeted issue) ───

test("processIssue with skipTeamCheck=true bypasses the team-author check entirely (hook effect)", async () => {
  const gh = fakeGh({ permission: "write" }); // would be skipped as a team author under the normal path
  gh.getPermission = async () => {
    throw new Error("must not be called when skipTeamCheck is true");
  };
  const anthropic = fakeAnthropic([JSON.stringify({ labels: ["bug"], spec_section: null })]);
  const gate = makeWriteGate(true);
  const issue = { number: 11, user: { login: "britfreed" }, title: "t", body: "b", labels: [] };
  const result = await processIssue(issue, {
    gh,
    anthropic,
    spec: { headingListText: "", sections: new Map() },
    owner: "o",
    repo: "r",
    writeGate: gate,
    labelCache: new Map(),
    log: () => {},
    skipTeamCheck: true,
  });
  assert.notEqual(result.outcome, "skipped-team-author");
  assert.equal(result.outcome, "processed");
  assert.match(result.teamVerdict, /skipped \(dry-run issue_numbers test hook/);
});

test("processIssue with skipTeamCheck omitted (default false) still enforces the team-author check", async () => {
  const gh = fakeGh({ permission: "admin" });
  const anthropic = fakeAnthropic([]);
  const gate = makeWriteGate(true);
  const issue = { number: 12, user: { login: "britfreed" }, title: "t", body: "b", labels: [] };
  const result = await processIssue(issue, {
    gh,
    anthropic,
    spec: { headingListText: "", sections: new Map() },
    owner: "o",
    repo: "r",
    writeGate: gate,
    labelCache: new Map(),
    log: () => {},
  });
  assert.equal(result.outcome, "skipped-team-author");
});

test("processIssue in dry-run mode never calls gh.addLabels/createLabel/createComment for real", async () => {
  const gh = fakeGh({ permission: "none" });
  const anthropic = fakeAnthropic([JSON.stringify({ labels: ["bug", "needs-repro"], spec_section: null })]);
  const gate = makeWriteGate(true);
  const issue = { number: 10, user: { login: "outsider" }, title: "t", body: "b", labels: [] };
  const result = await processIssue(issue, {
    gh,
    anthropic,
    spec: { headingListText: "x.md: ## X", sections: new Map() },
    owner: "o",
    repo: "r",
    writeGate: gate,
    labelCache: new Map(),
    log: () => {},
  });
  assert.equal(result.outcome, "processed");
  assert.equal(gh.addLabelsCalls.length, 0);
  assert.equal(gh.createCommentCalls.length, 0);
  assert.equal(gh.createLabelCalls.length, 0);
  assert.equal(gate.writesPerformed, 0);
  assert.ok(gate.summaryEvents.length > 0);
});
