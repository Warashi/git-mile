# Issue & Milestone Lifecycle Guide

This guide walks through a complete workflow for milestones and issues using
the enriched CLI commands introduced in M2. The steps assume a fresh repository
initialised via `git-mile init`.

## Milestone Flow

1. **Create the milestone with extended metadata**

   ```bash
   git-mile create milestone "Ship onboarding flow" \
     --description-file docs/onboarding/overview.md \
     --comment "Kickoff: align on success criteria" \
     --label roadmap --label onboarding
   ```

   The milestone is opened immediately. The JSON output (`--json`) includes the
   generated ID, status, and label set if you need to script against it.

2. **Append a follow-up comment**

   ```bash
   git-mile comment milestone <MILESTONE_ID> \
     --comment "Updated timeline: beta next week" \
     --message "share timeline update"
   ```

   Comments accept inline text, files (`--comment-file`), or an editor session
   (`--editor`). Use `--quote <COMMENT_ID>` to reply with context.

3. **Adjust labels**

   ```bash
   git-mile label milestone <MILESTONE_ID> --add release-ready --remove onboarding
   ```

   Combine `--add`, `--remove`, or `--set` to converge on the desired label set.

4. **Inspect the milestone**

   ```bash
   # rich textual output
   git-mile show <MILESTONE_ID> --limit-comments 5

   # structured data for tooling
   git-mile show <MILESTONE_ID> --json | jq '.stats.comment_count'
   ```

   The show view renders the description (Markdown-aware), lists the latest
   comments, and summarises metadata including label history.

## Issue Flow

1. **Create the issue**

   ```bash
   git-mile create issue "Audit onboarding copy" \
     --description "Track copy updates for the new flow" \
     --comment "Initial review covers welcome screens" \
     --label docs --label ux
   ```

   Supply `--draft` to track work before it is ready to execute.

2. **Collaborate via comments**

   ```bash
   git-mile comment issue <ISSUE_ID> --editor
   ```

   The editor template includes the resource header, current status, labels, and
   optional quoted comment. Pass `--no-editor` to force non-interactive usage.

3. **Update labels as work progresses**

   ```bash
   git-mile label issue <ISSUE_ID> --add ready-for-review --remove docs
   ```

   Use `--set` when synchronising to an external label taxonomy and `--clear`
   to remove all labels before reapplying.

4. **List issues with the new table view**

   ```bash
   git-mile list issue --columns id,title,status,labels,comments --long
   git-mile list issue --json | jq '.[] | {title, labels, latest_comment_excerpt}'
   ```

   `--long` appends the description preview and latest comment excerpt to the
   table output. The JSON payload includes `description_preview`,
   `comment_count`, and `label_events` for automation.

## Troubleshooting & Tips

* `GIT_MILE_LIST_LEGACY=1` restores the pre-M2 column layout while you migrate
  downstream tooling.
* `git-mile comment ... --dry-run` renders the comment body (with quoting) without
  persisting, useful for previewing Markdown tweaks.
* When scripting, capture IDs from JSON: \
  `MILE_ID=$(git-mile create milestone ... --json | jq -r '.id')`.
* All commands honour repository locking—long-running write operations will
  hold the lock until they complete; consider splitting large workflows into
  smaller steps if contention becomes an issue.
