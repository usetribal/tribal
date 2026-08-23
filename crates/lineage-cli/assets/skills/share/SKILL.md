---
name: share
description: >-
  Shares the current agent session as a public link with git-lineage. Use when
  the user asks to share this session, share this conversation, send someone
  this chat, or get a share link.
---

# Share this session

`tribal share` uploads the session you are in and prints a link anyone can
open — no account needed.

## Do this

Run from the repository root:

```bash
tribal share
```

Then give the user the printed URL, on its own, as the whole answer.

Only when the user names a different session than the one you are in:

```bash
tribal share --session <session-id>
```

If the command fails — not a lineage repo, not signed in, session marked
private — report what it said. Do not work around it; a private session is
meant to refuse.

## The link is a secret

Anyone holding the URL can read every shared turn. Treat it like a password:

- Paste the URL only where the user asked it to go.
- Never paste transcript content alongside it — the link already carries that.
- Do not put it in a commit message, a code comment, or a public issue.

The link is a snapshot: it shows the turns that existed when you ran the
command, and nothing you say afterwards.
