# Recipes

A recipe is a named, reviewed prompt plus a declared list of what it touches
(files, network, commands). Recipes come in two kinds — recipes that run as
an ongoing agent, in `recipes/agents/`, and recipes that run as a one-off
job, in `recipes/jobs/`.

## How to use one

1. Read `recipes/RECIPE-TEMPLATE.md` to see the shape every recipe takes.
2. Open the recipe you want and read the **What it touches** section first —
   know what you're agreeing to before you run anything.
3. Read the prompt itself, in full, not just the description.
4. Run it.

## Submitting a recipe

There are two ways:

- **Pull request (coders).** Copy `recipes/RECIPE-TEMPLATE.md` into the
  matching subfolder, fill it out, open a PR.
- **Issue form (non-coders).** Use the
  ["Submit a recipe"](https://github.com/ThreeLaneStudios/pane/issues/new?template=recipe-submission.yml)
  issue form. A maintainer turns the submission into a PR.

Either route goes through the same review before it merges — see
`recipes/REVIEW-CHECKLIST.md` for exactly what gets checked. The checklist is
public so you can run it on yourself first.

## Disclaimer

Recipes are community-submitted and reviewed before merge, but review
reduces risk, it doesn't eliminate it. Read any recipe fully before running
it. You run recipes at your own risk.
