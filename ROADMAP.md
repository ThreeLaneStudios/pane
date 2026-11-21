# Roadmap

What comes next, in the likely order. We build one item at a time; each gets a spec section and an ADR before build issues are filed.

1. **Events.** A trigger as a third kind of schedule: watch a folder, poll a feed (RSS, GitHub, X), let one agent raise an event for another, listen on a Discord socket. Two-stage filter before anything reaches a model; triggered turns withhold risky actions the way external content does. First because triggers are runtime work nothing else here provides.
2. **MCP client.** Connect any MCP server (HTTP and stdio) with OAuth and per-server scoping. Its tools land under the same gates as Pane's own.
3. **GitHub connection and Dev board.** Two tokens (yours for buttons, the agent's for tools), GitHub read tools for agents, then a pipeline board (Scoped, Building, Audit, Needs your label, Merged, Live-tested) whose approve and merge buttons stay greyed until the audited commit, green CI, and the label line up. First paid extension.
4. **Agent code work.** A sandboxed runner so agents build software from inside Pane: clone and worktree per job inside `~/Pane`, approved commands only, network allowlist, command output treated as untrusted, anything leaving the sandbox gated by you. Exposed through the MCP client so the executor is swappable.
5. **Agent browser.** Act on the web as you: read freely, act on consent. You sign in once in a Pane-owned browser profile; the agent drives it and never holds a password; submit, post, pay, and delete wait for your yes. Per-site allowlist, action log.
6. **Mail and calendar.** An email identity for your agent over IMAP, SMTP, and CalDAV: act on mail, send reminders, create events by `.ics` invite, trigger jobs from the inbox. After Events, because the inbox poll is one.
7. **Bookkeeping, then payments.** Double-entry bookkeeping as an extension, then payments on top.
8. **Multi-machine.** The same `~/Pane` on your laptop and your desktop, kept in sync end-to-end encrypted, with a sign-in that exists only to pair your own devices. The sync engine is already in the repo and tested; this turns it on.
9. **Mobile app.** Your notes, tasks, and agents on your phone through the same sync. Read and capture first, chat second.
10. **Teams.** People and their agents working with other people and their agents on a shared project: shared notes and tasks, each person's agents visible to the others as crew, project memory shared.

Questions about any of this go to the issue tracker or the [Pane Discord](https://discord.com/invite/PhRd2yGwR8).
