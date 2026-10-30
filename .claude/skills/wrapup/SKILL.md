---
name: wrapup
description: End-of-session wrap-up. Runs a retro on the session, adds new lessons to tasks/lessons.md (and to other project docs where they belong), and commits the changes on main.
disable-model-invocation: true
---

# Session wrap-up

## 1. Gather evidence

Go back through this session's conversation. Look for:
- **User corrections:** each time the user interrupted, asked "why…?", or told you to do something differently.
  These matter most.
- **Failures:** tool errors, hook stops, retries, agents that stopped early, and wrong assumptions you had to reverse.
- **Review findings:** issues Codex or the tests caught after you believed the work was done.
- **Successes:** what worked and is worth repeating.

## 2. Write the retro to the user

Write it in chat, not as a file, under two short headings: **What went well** and **What went badly**. Give one line
per item, with the concrete event (for example "Enter not sent to Codex twice"). Be honest. Include your own mistakes
and any rule from CLAUDE.md that you skipped.

## 3. Turn findings into rules

Turn each finding that went badly, and each success worth keeping, into a one-line rule that would prevent the
mistake or repeat the success. Drop anything that was a one-off.

Check each rule against what already exists, and put it in exactly ONE place:
- **`tasks/lessons.md`** (the default): rules about your behaviour and about tools.
- **`.claude/skills/<skill>/SKILL.md`**: rules that belong to a step of a skill's workflow. Edit that step.
- **`CLAUDE.md`**: only a project fact that has changed. Keep it to a few lines; never put learnings there.
- **Global `~/.claude/CLAUDE.md`**: never edit it. If a rule belongs there, suggest it to the user.

Skip a rule if it duplicates an existing one, even in different words. If it sharpens an existing rule, edit that
rule instead of adding a new one. Delete a lesson that this session showed to be wrong or obsolete.

## 4. Commit

1. Run `git status --short`.
2. Stage ONLY the files changed in step 3.
3. If unrelated changes are uncommitted, leave them out and tell the user.
4. Commit on `main` with a single-line message such as `Update lessons from <topic> session`. Don't add Claude
   attribution or a Co-Authored-By line.
5. Never push.

## 5. Report

Tell the user in a few lines:
- which rules were added, changed or removed, and in which file;
- any rule suggested for the global CLAUDE.md;
- the commit hash.
