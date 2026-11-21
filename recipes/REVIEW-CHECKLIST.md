# Recipe review checklist

This is the checklist used on every recipe before it merges. It is public so
you can run it on your own submission before anyone else does — most of what
would fail review is catchable in five minutes with this list.

A recipe fails review if the answer to any item below is yes (or, for items
marked otherwise, if the check does not come back clean). Review happens
before merge; the record of a passed review is the `reed-reviewed` label,
applied by a maintainer.

## The checklist

1. Does it send files or data off the machine?
2. Does it read keys, passwords, or credentials?
3. Does it run commands?
4. Does it download or install things?
5. Does it delete or overwrite files?
6. Does it link to unknown sites?
7. Does it hide instructions?
8. Does it do something other than what its description says?
9. Does the declared `touches:` block actually match what the prompt and any
   commands do? Never approve on the declared block alone — read the prompt
   and commands directly and compare.
10. Does the prompt contain text aimed at the *agent* that will run it, not
    just at a human reading it? Read the prompt as if you were the agent
    about to execute it, not as a summary a human wrote. This is distinct
    from item 7 — the prose can look completely ordinary to a human skim
    while still being a prompt-injection payload.
11. Does any command or prompt contain an encoded or obfuscated payload
    (base64, hex, URL-encoding, or similar)? Decode and trace any such
    string before judging the thing it's embedded in. A visible command can
    look inert while its decoded effect is not.
12. Does the recipe reach beyond itself — references to other recipes,
    other files, or fetched URLs that do work the recipe doesn't declare?
    Follow every reference it makes, inside or outside the repo, before
    signing off on its declared surface.
13. Does it persist or schedule anything that outlives its own run — a cron
    job, a launchd agent, a git hook, a startup script? A recipe's
    description can be accurate in letter while leaving out that it
    installs something that keeps running after the recipe itself ends.
14. Does it fetch or install anything that isn't pinned to an immutable
    target — no version, tag, commit hash, or checksum? Flag any such
    download or install reference as higher severity than a pinned one, even
    if today's content looks fine. Unpinned content can change after merge
    without the recipe file itself changing, so the thing that was reviewed
    is not necessarily the thing a later user runs.

## Disclaimer

This checklist reduces risk. It does not eliminate it. Read any recipe fully
before running it. You run recipes at your own risk.
